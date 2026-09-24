use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};
use crate::Chain;

const CHAR_HAIR_GEOSETS: &str = "DBFilesClient\\CharHairGeosets.dbc";
const CHAR_FACIAL_HAIR: &str = "DBFilesClient\\CharacterFacialHairStyles.dbc";
const HELMET_GEOSET_VIS: &str = "DBFilesClient\\HelmetGeosetVisData.dbc";

/// The default 16 region-base geosets (`cc+0x144`, set by the ctors `0x476810`/`0x476960`) —
/// increments of 100 (group 7's base is the outlier 702). Entries 0–3 (hair + the 3 facial-hair
/// groups) are overwritten by the customization; 4–15 (the equipment-group bases:
/// glove/boot/sleeve/…/cloak) stay as these bare-skin defaults.
const REGION_BASES: [u16; 16] = [
    1, 101, 201, 301, 401, 501, 601, 702, 801, 901, 1001, 1101, 1201, 1301, 1401, 1501,
];

/// The customization → geoset tables, resolving an appearance to the set of visible geosets.
pub struct CharacterGeosets {
    /// (race, sex, hairStyle) → hair `GeosetID` (group 0).
    hair: HashMap<(u8, u8, u8), u32>,
    /// (race, sex, facialHair) → the 3 facial-hair geoset variations (DBC fields gA/gB/gC, in file
    /// order — gA→group 1 (+100), gB→group 3 (+300), gC→group 2 (+200), per `0x478660`).
    facial: HashMap<(u8, u8, u8), [u32; 3]>,
    /// HelmetGeosetVisData row id → its 5 **race** bitmasks (consumer `0x4799a0`): a set bit
    /// `1 << race` in column *c* forces one region-base slot back to its group base — hiding the
    /// styled hair/facial geoset (and the ears) under a worn helm.
    helmet_vis: HashMap<u32, [u32; 5]>,
}

/// The worn geoset selectors the equipment branches of `0x477520` read (decisions 0074, 1864): per
/// bodyslot 2–9 (shirt, chest, belt, pants, boots, wrist, gloves, tabard) the item's
/// `geosetGroup[0..2]` (`None` = the slot is empty), plus the cloak's `geosetGroup[0]`, plus the
/// worn helm's `HelmetGeosetVisData` row pair (`[male, female]` — ItemDisplayInfo cols 12/13).
/// Default = naked.
///
/// **Two of the eight branches are not fed by an ItemDisplayInfo column at all** and take their own
/// field here (decision 1864): B3 reads the ArmLower *composite grid*, and B6 reads a client flag.
#[derive(Default, Clone, Copy)]
pub struct EquipGeosets {
    pub bodyslots: [Option<[u32; 3]>; 8],
    pub cloak: Option<u32>,
    pub helm_vis: Option<[u32; 2]>,
    /// B3's gate: does anything **other than the shirt** dress the forearm? — the six dwords
    /// `cc+0x26c..cc+0x280` (ArmLower cells 1..6). Fill it from
    /// [`forearm_dressed`](crate::forearm_dressed), which reads the one composite plan.
    pub forearm_dressed: bool,
    /// B6's gate: `[cc+0xc]`, "the guild registrar's tabard designer is open on this character"
    /// (its setter `0x5e07fb`). Set while that window is up so the body wears a previewable tabard
    /// with an **empty** tabard slot; nothing else in the client ever sets it.
    pub tabard_preview: bool,
}

impl CharacterGeosets {
    /// The geoset IDs a character of this appearance renders — the opening block (the naked set)
    /// plus **all eight** equipment branches B1–B8 of `0x477520` (decisions 0074, 1864 —
    /// implemented from its *arithmetic*; the branch comments below name each one at its address).
    /// A body submesh whose `skinSectionId` is in this set is drawn; all others hidden. Most
    /// branches gate on an ItemDisplayInfo `geosetGroup` field being non-zero ("section present");
    /// B3 and B6 do not, and read [`EquipGeosets::forearm_dressed`] / [`EquipGeosets::tabard_preview`].
    ///
    /// Returned **sorted and deduplicated**: the client toggles a per-submesh flag, where several
    /// branches re-enable a base the opening loop already set, so a list needs normalising once.
    pub fn visible_geosets(
        &self,
        race: u8,
        sex: u8,
        hair_style: u8,
        facial_hair: u8,
        equip: &EquipGeosets,
    ) -> Vec<u16> {
        let mut set = REGION_BASES.to_vec();
        set.push(0); // geoset 0 (the body), enabled unconditionally
                     // Entry 0 (group 0 = hair): the chosen hairstyle's geoset, clamped ≥1 (`0x478540`
                     // returns `max(1, geosetId)`, so a bald style still resolves to geoset 1, the scalp).
        if let Some(&g) = self.hair.get(&(race, sex, hair_style)) {
            set[0] = g.max(1) as u16;
        }
        // Entries 1–3 (groups 1/2/3 = facial hair): DBC variation + the group base (`0x478660` adds
        // 100/300/200 to fields gA/gB/gC respectively). Absent row ⇒ the default bases 101/201/301 stand.
        if let Some(&[a, b, c]) = self.facial.get(&(race, sex, facial_hair)) {
            set[1] = (a + 100) as u16; // gA → group 1
            set[3] = (b + 300) as u16; // gB → group 3
            set[2] = (c + 200) as u16; // gC → group 2
        }
        // A worn helm's vis row (`0x4799a0`): each of the 5 columns is a
        // **race** bitmask; a set `1 << race` bit forces one region-base slot back to its group
        // base, dropping the styled geoset the customization just selected — hair to the bare scalp
        // (1), the facial groups to their bases, and the ears to **701** (over the 702 default no
        // customization touches). Runs after the customization overwrites, exactly the client's
        // order; the slots {0,1,2,3,7} are disjoint from every equipment branch below.
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
        // The equipment branches. `g(slot, sub)` = the slot's geosetGroup[sub], gated non-zero.
        // EquipGeosets bodyslot indices: 0 shirt · 1 chest · 2 belt · 3 pants · 4 boots · 5 wrist ·
        // 6 gloves · 7 tabard.
        let g = |slot: usize, sub: usize| {
            equip.bodyslots[slot]
                .map(|groups| groups[sub])
                .filter(|v| *v != 0)
        };
        let disable = |set: &mut Vec<u16>, lo: u16, hi: u16| set.retain(|id| *id < lo || *id > hi);
        // The robe bit: chest or pants geosetGroup[2] (B4's guard) — it also suppresses B5's tabard
        // flap and B6's preview. B7 is NOT gated on it: that branch reads the chest's bit alone.
        let robe = g(1, 2).or_else(|| g(3, 2));
        // B1/B2 (`0x477564`): gloves replace the glove group (401+v, own range disabled); else the
        // chest's sleeves (the `else` arm, B2).
        if let Some(v) = g(6, 0) {
            disable(&mut set, 401, 499);
            set.push(401 + v as u16);
        } else if let Some(v) = g(1, 0) {
            set.push(801 + v as u16);
        }
        // B3 (`0x4775bd`): the shirt's sleeve cuff — only when nothing ELSE dresses the forearm.
        // The reference's gate is the ArmLower equip row's cells 1..6, not "is a chest equipped":
        // most chest pieces leave that tile empty and the cuff survives them, while a bracer or a
        // glove takes it and the cuff goes (decision 1864; [`EquipGeosets::forearm_dressed`]).
        if !equip.forearm_dressed {
            if let Some(v) = g(0, 0) {
                set.push(801 + v as u16);
            }
        }
        // B4 (`0x4775fb`): a robe hides boots/kneepads/pant-legs/trousers and shows its skirt
        // (1301+v); else the boots replace the boot group; else the pants' kneepads. The bare `901`
        // enables are the reference's own (`0x477648`, `0x477685`) and are no-ops here — the 16-base
        // loop already carries 901 and nothing on this path disables group 9 — kept so the sequence
        // stays the reference's rather than resting on that invariant.
        if let Some(v) = robe {
            disable(&mut set, 501, 599);
            disable(&mut set, 902, 999);
            disable(&mut set, 1100, 1199);
            disable(&mut set, 1300, 1399);
            set.push(1301 + v as u16);
        } else if let Some(v) = g(4, 0) {
            // `0x477639` disables the WHOLE boot group before enabling the boot's own geoset — the
            // naked foot comes off with the boot on. 501 is a real submesh on 11 of the 18 character
            // models (64 vertices on Human male), so leaving it on — as benilla did until decision
            // 1864, on a misread of this same note — put a bare ankle inside every boot.
            disable(&mut set, 501, 599);
            set.push(901);
            set.push(501 + v as u16);
        } else if let Some(v) = g(3, 1) {
            set.push(901 + v as u16);
        } else {
            set.push(901);
        }
        // B5 (`0x477706`): the tabard flap (robes hide it).
        if robe.is_none() {
            if let Some(v) = g(7, 0) {
                set.push(1201 + v as u16);
            }
        }
        // B6 (`0x477752`): the tabard-designer preview. **No ItemDisplayInfo column feeds this
        // branch** — its gate is the flag `[cc+0xc]`, set only while the guild registrar's tabard
        // designer is open (setter `0x5e07fb`; the branch's old "helm-skirt" label was a mislabel,
        // and no shipped head display carries a geoset group at all). It forces the tabard group on
        // with an EMPTY tabard slot: 1201, plus 1202 when neither the chest nor the legs carries the
        // robe bit. On shipped character models 1201 has no submesh (it is the group's "no tabard"
        // base) and 1202 is the flap — so the observable effect is exactly "wear the tabard so the
        // design can be previewed" (decision 1864).
        if equip.tabard_preview {
            set.push(1201); // already on from the opening loop; the reference enables it anyway
            if robe.is_none() {
                set.push(1202);
            }
        }
        // B7 (`0x477799`): the shirt's doublet + the pants' leg geoset (the recorded 1102 base — not
        // 1101). Skipped ENTIRELY when the CHEST is a robe (`0x4777a4` — the legs' own robe bit does
        // NOT gate here, unlike B4/B5/B6) or when a TABARD is worn (`0x4777b8`, bodyslot 9 sub 0;
        // the guard is the tabard, not the glove). Decision 1864.
        if g(1, 2).is_none() && g(7, 0).is_none() {
            if let Some(v) = g(0, 1) {
                set.push(1001 + v as u16);
            }
            if let Some(v) = g(3, 0) {
                set.push(1102 + v as u16);
            }
        }
        // B8 (`0x47780a`): a cloak replaces the cloak group (1501+v, range disabled).
        if let Some(v) = equip.cloak.filter(|v| *v != 0) {
            disable(&mut set, 1500, 1599);
            set.push(1501 + v as u16);
        }
        // The client's `0x7110d0` writes a flag per submesh, so enabling an already-enabled geoset
        // is idempotent there; ours is a list, and several branches re-enable a base the opening
        // loop already added. Normalise once at the end so the result is a canonical set.
        set.sort_unstable();
        set.dedup();
        set
    }

    /// Load both customization DBCs from the patch chain.
    ///
    /// **A duplicated `(race, sex, variation)` key resolves to the FIRST row** (decision 0682). Both
    /// consumers are *linear scans* of the loaded table — `0x478540` for the hair geoset, `0x478740`
    /// for the facial-hair record (`0x478540` is a loaded-DBC linear scan, not a pool) — so the
    /// earliest matching record is the one the reference returns, where a last-write
    /// `HashMap::insert` would take the latest.
    ///
    /// It matters exactly once in the shipped data, and it was B11: `CharHairGeosets` carries **four**
    /// rows for **(race 9 goblin, sex 0, variation 0)** — ids 241–244, geosets 1 / 2 / 1 / 2, the only
    /// duplicated key in the file (243/244 read as the goblin-*female* pair with a zeroed sex column,
    /// which is why goblin females, having no row at all, were always right). Geoset 1 is the bare
    /// scalp; geoset 2 is that scalp **plus** a 38-vertex topknot whose texture slot is `Monster1` —
    /// a slot no character-model goblin display fills, so it drew flat white (decision 0157). All 314
    /// character-model goblin displays select variation 0, so last-write put a white tuft on every one
    /// of them. `CharacterFacialHairStyles` has no duplicate keys today; it takes the same rule
    /// because the same scan law governs it.
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
                    // FIRST row wins — see the duplicate-key note on [`CharacterGeosets::load`].
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
                // fields: RaceID, SexID, VariationID(facialHair), 3×unused, gA, gB, gC (no ID column).
                if let (Some(race), Some(sex), Some(var)) =
                    (u32_at(r, 0), u32_at(r, 1), u32_at(r, 2))
                {
                    let g = [
                        u32_at(r, 6).unwrap_or(0),
                        u32_at(r, 7).unwrap_or(0),
                        u32_at(r, 8).unwrap_or(0),
                    ];
                    // FIRST row wins — see the duplicate-key note on [`CharacterGeosets::load`].
                    m.entry((race as u8, sex as u8, var as u8)).or_insert(g);
                }
            }
            m
        };
        // HelmetGeosetVisData — the helm hide-masks (read by `0x4799a0`). Soft-optional: a
        // missing/undecodable table just means helms never hide hair (the pre-helm-vis behavior),
        // like the other soft-fails.
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

/// CharHairGeosets.dbc — 6 fields in build 5875 (verified against the file header).
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

/// HelmetGeosetVisData.dbc — 6 fields in build 5875 (verified: the loader `0x546f00` asserts
/// fieldCount 6 / recordSize 0x18; the 5 mask columns are **race** bitmasks, as `0x4799a0` tests
/// them).
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

/// CharacterFacialHairStyles.dbc — 9 fields in build 5875 (verified); no ID column (keyed race/sex/var).
pub(crate) fn char_facial_hair_schema() -> Schema {
    let mut s = Schema::new("CharacterFacialHairStyles");
    for (name, ty) in [
        ("RaceID", FieldType::UInt32),
        ("SexID", FieldType::UInt32),
        ("VariationID", FieldType::UInt32),
        ("Unused3", FieldType::UInt32),
        ("Unused4", FieldType::UInt32),
        ("Unused5", FieldType::UInt32),
        ("Geoset100", FieldType::UInt32), // gA → group 1 (+100)
        ("Geoset300", FieldType::UInt32), // gB → group 3 (+300)
        ("Geoset200", FieldType::UInt32), // gC → group 2 (+200)
    ] {
        s.add_field(SchemaField::new(name, ty));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The equipment geoset branches (decisions 0074/1864, from the arithmetic of `0x477520`):
    /// gloves/boots/robe/cloak **replace** their groups, and a robe suppresses the tabard branch.
    #[test]
    fn equipment_geoset_branches() {
        let cg = CharacterGeosets {
            hair: HashMap::new(),
            facial: HashMap::new(),
            helmet_vis: HashMap::new(),
        };
        let naked = cg.visible_geosets(1, 0, 0, 0, &EquipGeosets::default());
        assert!(naked.contains(&401) && naked.contains(&1101) && naked.contains(&1501));

        // Gloves v=1: the glove group is replaced (401 out, 402 in).
        let mut eq = EquipGeosets::default();
        eq.bodyslots[6] = Some([1, 0, 0]);
        let set = cg.visible_geosets(1, 0, 0, 0, &eq);
        assert!(set.contains(&402) && !set.contains(&401));

        // Boots v=2: the boot group is replaced — 503 in, the naked foot 501 OUT (`0x477639`
        // disables [501,599] first). benilla stacked them until decision 1864.
        let mut eq = EquipGeosets::default();
        eq.bodyslots[4] = Some([2, 0, 0]);
        let set = cg.visible_geosets(1, 0, 0, 0, &eq);
        assert!(set.contains(&503) && !set.contains(&501));

        // A robe (chest group[2]=1) shows its skirt (1302) and hides boots/pant-legs/trousers —
        // including a worn tabard's flap and the pants' own branches.
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

        // Cloak v=4: the cloak group is replaced (1501 out, 1505 in).
        let eq = EquipGeosets {
            cloak: Some(4),
            ..Default::default()
        };
        let set = cg.visible_geosets(1, 0, 0, 0, &eq);
        assert!(set.contains(&1505) && !set.contains(&1501));
    }

    /// **B3's real gate** (`0x4775bd`, decision 1864): the shirt's sleeve cuff survives or dies by
    /// what dresses the *forearm tile*, not by whether a chest is equipped. Both directions matter —
    /// 533 of the 637 shipped chest displays leave `ArmLowerTexture` empty, and 566 of 570 wrist
    /// displays fill it — and benilla had both backwards until this record.
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
        // And a chest in the slot is NOT the gate: a chest whose ArmLower art is empty leaves the
        // tile free, and the cuff stays. (The chest's own sleeve geoset is B1's `else`, group 8 too,
        // so give this one none.)
        let mut eq = shirt(false);
        eq.bodyslots[1] = Some([0, 0, 0]);
        assert!(
            cg.visible_geosets(1, 0, 0, 0, &eq).contains(&802),
            "an equipped chest alone does not hide the shirt's cuff"
        );
    }

    /// **B6 — the tabard-designer preview** (`0x477752`; decision 1864). The one
    /// branch of the eight with no ItemDisplayInfo column behind it: its gate is `[cc+0xc]`, "the
    /// guild registrar's tabard designer is open", and it wears the tabard flap (1202) with an
    /// **empty** tabard slot — unless a robe is on, which suppresses it exactly as it suppresses B5.
    #[test]
    fn b6_tabard_preview_wears_the_flap_with_an_empty_slot() {
        let cg = CharacterGeosets {
            hair: HashMap::new(),
            facial: HashMap::new(),
            helmet_vis: HashMap::new(),
        };
        // Off: an empty tabard slot shows no flap.
        let set = cg.visible_geosets(1, 0, 0, 0, &EquipGeosets::default());
        assert!(!set.contains(&1202), "no tabard, no designer: no flap");
        // On: the flap appears with nothing in the slot.
        let preview = EquipGeosets {
            tabard_preview: true,
            ..Default::default()
        };
        let set = cg.visible_geosets(1, 0, 0, 0, &preview);
        assert!(set.contains(&1202), "the designer forces the flap on");
        assert!(set.contains(&1201), "and the group base with it");
        // A robe suppresses it, from either the chest's or the legs' robe bit.
        for slot in [1usize, 3] {
            let mut eq = preview;
            eq.bodyslots[slot] = Some([0, 0, 1]);
            assert!(
                !cg.visible_geosets(1, 0, 0, 0, &eq).contains(&1202),
                "a robe (slot {slot}) hides the previewed flap"
            );
        }
    }

    /// **B7's two guards** (`0x4777a4` / `0x4777b8`, decision 1864): the shirt-doublet + pant-leg arm
    /// is skipped whole when the CHEST is a robe or when a TABARD is worn — and, unlike B4/B5/B6,
    /// the *legs'* own robe bit does not gate it.
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

        // A worn tabard skips the whole branch (`0x4777b8` guards on the tabard, not the glove).
        let mut eq = base;
        eq.bodyslots[7] = Some([1, 0, 0]);
        let set = cg.visible_geosets(1, 0, 0, 0, &eq);
        assert!(set.contains(&1202), "the tabard's own flap is on");
        assert!(
            !set.contains(&1002) && !set.contains(&1103),
            "a tabard skips B7 whole"
        );

        // A chest robe skips it too — and takes the doublet with it, which the pre-1864 code left on.
        let mut eq = base;
        eq.bodyslots[1] = Some([0, 0, 1]);
        let set = cg.visible_geosets(1, 0, 0, 0, &eq);
        assert!(
            !set.contains(&1002) && !set.contains(&1103),
            "a chest robe skips B7 whole"
        );

        // The LEGS' robe bit does not: B4-long still hides the pant-leg group, but the doublet —
        // enabled after that disable — stays on. This is the asymmetry the old `robe` shortcut lost.
        let mut eq = base;
        eq.bodyslots[3] = Some([1, 0, 1]);
        let set = cg.visible_geosets(1, 0, 0, 0, &eq);
        assert!(set.contains(&1002), "leg robes do not gate B7");
        assert!(
            set.contains(&1103),
            "and B7 re-enables the pant leg after B4-long"
        );
    }

    /// The helm-vis force (`0x4799a0`): each vis column is a **race** bitmask; a set
    /// `1 << race` bit forces the slot back to its group base — hair to the bare scalp (1), facial
    /// groups to 101/201/301, ears to 701 — dropping the styled geosets. The row is sex-selected
    /// (ItemDisplayInfo col 12 male / 13 female); a mask without our race bit leaves the style.
    #[test]
    fn helm_vis_forces_group_bases_by_race() {
        let mut hair = HashMap::new();
        hair.insert((1, 0, 1), 5u32); // styled hair → geoset 5
        let mut facial = HashMap::new();
        facial.insert((1, 0, 1), [2u32, 3, 4]); // styled facial → 102/303/204
        let mut helmet_vis = HashMap::new();
        helmet_vis.insert(368, [u32::MAX; 5]); // hide-everything row (real id, dumped)
        helmet_vis.insert(245, [0u32; 5]); // hide-nothing row (real id, dumped)
        let cg = CharacterGeosets {
            hair,
            facial,
            helmet_vis,
        };
        let helm = EquipGeosets {
            helm_vis: Some([368, 245]),
            ..Default::default()
        };
        // Male (sex 0) picks row 368: everything forced to base.
        let set = cg.visible_geosets(1, 0, 1, 1, &helm);
        assert!(set.contains(&1) && !set.contains(&5), "hair → bare scalp");
        assert!(set.contains(&101) && !set.contains(&102));
        assert!(set.contains(&201) && !set.contains(&204));
        assert!(set.contains(&301) && !set.contains(&303));
        assert!(set.contains(&701) && !set.contains(&702), "ears → 701");
        // The female column of this pair is the hide-nothing row: styles survive.
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

    /// The naked selection for a known Human-male appearance (rows dumped from the build-5875 files):
    /// hairStyle 1 → GeosetID 2; facialHair 1 → gA/gB/gC = 1/2/1. So the set carries the body (0), the
    /// hair geoset (2), the three facial geosets (101 / 201 / 302), and the default group bases.
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
        // Default equipment-group bases (entries 4–15) stand for a naked body.
        assert!(set.contains(&401) && set.contains(&702) && set.contains(&1501));
        // A non-selected hair variant is hidden (geoset 5 is hairStyle 4, not chosen).
        assert!(!set.contains(&5), "unselected hair variant hidden");
    }

    /// A bald hairstyle (GeosetID 0) still resolves to geoset 1 (the scalp), never 0-the-body-collision.
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

    /// **B11, against the shipped DBC** (decision 0682): `CharHairGeosets` carries four rows for the
    /// goblin male's only hairstyle, and the reference's linear scan takes the first — geoset **1**,
    /// the bare scalp. Last-write took id 244's geoset **2**, which adds an untextured topknot.
    ///
    /// Three things are pinned here because each on its own could rot: that the shipped file really
    /// does duplicate that key (the defect only exists if it does), that we resolve it to 1, and that
    /// **no other key in the table is duplicated** — which is what bounds this change to goblins.
    #[test]
    fn goblin_male_hair_takes_the_first_of_four_duplicate_rows() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");

        // The file's own rows, read independently of the loader, so the test states the premise it
        // rests on rather than assuming it.
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

        // And the loader resolves it the reference's way.
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
        // The goblin female has no row at all (race 9 ships only sex-0 rows), so she falls to the
        // region base, which is the same scalp. She was never affected; assert it so a future
        // "fix" to the sex column has to notice her.
        let f = cg.visible_geosets(9, 1, 0, 0, &EquipGeosets::default());
        assert!(f.contains(&1) && !f.contains(&2), "goblin female unchanged");
    }
}
