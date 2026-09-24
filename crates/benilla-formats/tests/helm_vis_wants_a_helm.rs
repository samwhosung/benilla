//! B93 — a `HelmetGeosetVisData` hide mask only applies when the head display is a **worn helm**,
//! i.e. it names a model. Real-data difftest against the shipped 1.12.1 DBCs, end to end from the
//! NPC's `CreatureDisplayInfoExtra` row to the geosets her body renders. Skips when the client
//! isn't present.
//!
//! The reported case is **Jubie Gadgetspring** (vmangos creature 8678 → `CreatureDisplayInfo`
//! 7969 → extra **5503**): a gnome female whose head column names display **15676**, an
//! `INV_Jewelry_Amulet_01` row with **no model** and a full hide mask (`HelmVisFemale` **306** =
//! `[446, 478, 510, 222, 238]`, every column carrying the gnome bit `1 << 7`). 1.12.1 renders her
//! pigtails, her long ears and her earrings; honouring the mask strips all three.

use benilla_formats::{
    load_creature_catalog, load_item_display_catalog, open_chain, CharacterGeosets, EquipGeosets,
};

/// Jubie's `CreatureDisplayInfo` id — the chain's entry point.
const JUBIE_DISPLAY: u32 = 7969;
/// The amulet-shaped `ItemDisplayInfo` row her head column names.
const AMULET_DISPLAY: u32 = 15676;

#[test]
fn a_modelless_head_display_is_not_a_worn_helm() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let items = load_item_display_catalog(&mut chain).expect("ItemDisplayInfo");

    // 1. The defect only exists if the shipped row really is model-less AND masked.
    let amulet = items.get(AMULET_DISPLAY).expect("display 15676 exists");
    assert!(
        amulet.model.iter().all(Option::is_none),
        "display {AMULET_DISPLAY} names no model in either slot, got {:?}",
        amulet.model
    );
    assert_eq!(
        amulet.helmet_vis,
        [248, 306],
        "display {AMULET_DISPLAY} still carries a HelmetGeosetVisData pair"
    );
    // 2. …and we now refuse to read it as a helm.
    assert_eq!(
        amulet.worn_helm_vis(),
        None,
        "a model-less display is not a worn helm"
    );

    // 3. A real helm still hides what it should — the change must not disarm the mechanism.
    let (_, helm) = items
        .iter()
        .find(|(_, d)| d.model[0].is_some() && d.helmet_vis != [0, 0])
        .expect("some display is a modelled helm with a vis row");
    assert_eq!(
        helm.worn_helm_vis(),
        Some(helm.helmet_vis),
        "a modelled helm keeps its vis pair"
    );

    // 4. The blast radius, pinned: of the 1314 rows that author a vis pair, exactly 12 leave the
    //    LEFT model slot empty. If the shipped data ever changes, this test says so rather than the
    //    change silently growing.
    let masked = items.iter().filter(|(_, d)| d.helmet_vis != [0, 0]).count();
    let masked_modelless = items
        .iter()
        .filter(|(_, d)| d.helmet_vis != [0, 0] && d.model[0].is_none())
        .count();
    assert_eq!((masked, masked_modelless), (1314, 12));

    // 5. The gate is `ModelName[0]` alone (`0x4799c1`), not "either slot". 41 rows
    //    fill only the RIGHT slot; none of them carries a vis pair, so the two readings coincide on
    //    the shipped table — assert that rather than assume it, because the day one diverges the
    //    reference's answer is the left slot's.
    let right_only = items
        .iter()
        .filter(|(_, d)| d.model[0].is_none() && d.model[1].is_some())
        .count();
    let right_only_masked = items
        .iter()
        .filter(|(_, d)| d.model[0].is_none() && d.model[1].is_some() && d.helmet_vis != [0, 0])
        .count();
    assert_eq!((right_only, right_only_masked), (41, 0));
}

#[test]
fn jubie_keeps_her_hair_ears_and_earrings() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let creatures = load_creature_catalog(&mut chain).expect("creature catalog");
    let items = load_item_display_catalog(&mut chain).expect("ItemDisplayInfo");
    let geosets = CharacterGeosets::load(&mut chain).expect("customization tables");

    let model = creatures
        .model(JUBIE_DISPLAY)
        .expect("display 7969 resolves a model");
    let npc = model
        .npc_appearance
        .as_ref()
        .expect("display 7969 has a CreatureDisplayInfoExtra row");
    assert_eq!(
        (npc.race, npc.sex, npc.hair_style, npc.facial_hair),
        (7, 1, 1, 2),
        "gnome female, hairstyle 1, facial-hair (earring) variation 2"
    );
    assert_eq!(
        npc.equipment[0], AMULET_DISPLAY,
        "her head column is the model-less amulet row"
    );

    let head = items.get(npc.equipment[0]).expect("head display row");
    let eg = EquipGeosets {
        helm_vis: head.worn_helm_vis(),
        ..EquipGeosets::default()
    };
    let set = geosets.visible_geosets(npc.race, npc.sex, npc.hair_style, npc.facial_hair, &eg);
    // CharHairGeosets (7,1,1) → 3 · CharacterFacialHairStyles (7,1,2) geoset200 3 → 203 · the
    // ear region keeps its 702 default. All three are what the reference shows.
    for want in [3u16, 203, 702] {
        assert!(set.contains(&want), "geoset {want} is on, got {set:?}");
    }
    for hidden in [1u16, 201, 701] {
        assert!(
            !set.contains(&hidden),
            "geoset {hidden} (the helm-tucked variant) is off, got {set:?}"
        );
    }

    // The counterfactual — the shipped mask, honoured — is exactly the reported picture: bare
    // scalp, no earrings, tucked ears. This is what the bug looked like, asserted so a regression
    // has to walk past it.
    let broken = geosets.visible_geosets(
        npc.race,
        npc.sex,
        npc.hair_style,
        npc.facial_hair,
        &EquipGeosets {
            helm_vis: Some(head.helmet_vis),
            ..EquipGeosets::default()
        },
    );
    for want in [1u16, 201, 701] {
        assert!(broken.contains(&want), "the mask, honoured, forces {want}");
    }
}
