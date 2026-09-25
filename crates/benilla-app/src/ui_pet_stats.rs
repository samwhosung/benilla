//! The hunter pet's paper-doll stat block (`GetPetHappiness`, `GetPetLoyalty`,
//! `GetPetTrainingPoints`, `GetPetExperience`, `HasPetUI`) and the pet's family word and diet.
//!
//! The four stat bindings share one gate, `0x6116e0(pet)`: the pet resolves, has a
//! `UNIT_FIELD_PETNUMBER`, is ours, and our class byte is Hunter, so a warlock's imp answers
//! nothing. `UnitCreatureFamily("pet")` (`0x51a310`) has no class test, so the imp still shows
//! "Imp", while `GetPetFoodTypes()` (`0x4bea10`) shares the `0x6116e0` gate, so a non-hunter's pet
//! has no diet even when its family row has a food mask.

use bevy::prelude::*;

use benilla_ui::script::{PetStats, ScriptValue, UiScript};

use crate::names::NameCache;
use crate::net::{NetCommands, ObjectStore};
use crate::ui_pet::{PetBar, PetUnit};
use crate::ui_unit::UnitFeed;

/// `UNIT_FIELD_BYTES_0` byte 1, Hunter: the class the stat gate tests (`0x611752`). The enum is
/// pinned by `GetComboPoints` (`0x51a190`), which tests 4 and 0xB, Rogue and Druid, on that byte.
const CLASS_HUNTER: u8 = 3;

/// Power index 4, the happiness `GetPetHappiness` buckets: read by index, as a pet's displayed
/// power is focus.
const POWER_HAPPINESS: u8 = 4;

/// `PetPersonality.dbc` and `PetLoyalty.dbc`, loaded once; without them happiness and loyalty
/// answer the bindings' failure forms.
#[derive(Resource)]
pub(crate) struct PetStatTables {
    pub(crate) personalities: benilla_formats::PetPersonalities,
    pub(crate) loyalty: benilla_formats::PetLoyaltyNames,
}

/// `CreatureFamily.dbc` (the word and icon) and `ItemPetFood.dbc` (the diet a family's food mask
/// names), apart from [`PetStatTables`] so each half degrades alone.
#[derive(Resource)]
pub(crate) struct PetFamilyTables {
    pub(crate) families: benilla_formats::CreatureFamilies,
    pub(crate) foods: benilla_formats::PetFoodNames,
}

/// The pet snapshot's push; anything that fires a pet event orders after it. `fire_event` runs Lua
/// handlers synchronously and `PetTab_Update` tests only `HasPetUI()`, so a `UNIT_PET` fired
/// before the push keeps the Pet tab hidden for the session.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct PetSnapshot;

pub(crate) struct UiPetStatsPlugin;

impl Plugin for UiPetStatsPlugin {
    fn build(&self, app: &mut App) {
        // In the unit feed, so the pet frame repaints in the pass that pushes its health.
        app.add_systems(Update, feed_pet_stats.in_set(UnitFeed).in_set(PetSnapshot));
    }
}

/// The pet's family word, icon and diet, from its cached creature template. The template entry is
/// the descriptor's `OBJECT_FIELD_ENTRY`, not the guid's entry slot, which holds the pet number
/// (vmangos `Pet::Create`); `Creature::InitEntry` writes the real entry (`Creature.cpp:376`). A
/// miss sends the ask-once creature query and answers all-absent until it lands. The icon is
/// `CreatureFamily.dbc`'s own column: `GetPetIcon`'s answer and the stable's slot art.
fn family_for(
    pet: Option<&ObjectStore>,
    names: &NameCache,
    commands: &NetCommands,
    tables: Option<&PetFamilyTables>,
) -> (Option<String>, Option<String>, Vec<String>) {
    let Some(entry) = pet.and_then(|s| s.0.object_entry()).filter(|&e| e != 0) else {
        return (None, None, Vec::new());
    };
    // vmangos answers a creature query off the entry alone (`HandleCreatureQueryOpcode`), so the
    // guid is 0.
    let _ = names.resolve_creature(entry, 0, commands);
    let Some(tables) = tables else {
        return (None, None, Vec::new());
    };
    let Some(family_id) = names.creature_record(entry).map(|r| r.pet_family) else {
        return (None, None, Vec::new());
    };
    let Some(family) = tables.families.get(family_id) else {
        return (None, None, Vec::new());
    };
    (
        Some(family.name.clone()),
        tables.families.icon(family_id).map(str::to_string),
        tables
            .foods
            .for_mask(family.pet_food_mask)
            .into_iter()
            .map(str::to_string)
            .collect(),
    )
}

/// The whole stat block for the current pet, or [`PetStats::default`] with none.
fn stats_for(
    pet: Option<&ObjectStore>,
    self_store: Option<&ObjectStore>,
    tables: Option<&PetStatTables>,
    family: (Option<String>, Option<String>, Vec<String>),
) -> (bool, PetStats) {
    let (family, icon, food_types) = family;
    let Some(fields) = pet.map(|s| &s.0) else {
        return (false, PetStats::default());
    };
    // `HasPetUI`'s first return (`0x4be697`): a pet that resolves and has a pet number. A
    // possessed creature has a bar and no pet number, so no paper doll.
    let has_ui = fields.unit_is_pet_or_charm();
    // Its second return is our class; the owner leg always holds, as the server gives us a bar
    // only for a pet we control.
    let hunter = has_ui
        && self_store.map(|s| ((s.0.unit_bytes_0().unwrap_or(0) >> 8) & 0xff) as u8)
            == Some(CLASS_HUNTER);
    if !hunter {
        // The family word passes the gate and the diet does not (module doc): a mind-controlled
        // boar under a non-hunter (family 5, food mask 63) gets "Boar" and no diet.
        return (
            has_ui,
            PetStats {
                family,
                // The icon passes with the word; whether the reference gates it is untraced.
                icon,
                ..PetStats::default()
            },
        );
    }
    let (happiness, damage_percentage, loyalty_rate) = tables
        .and_then(|t| t.personalities.for_pet(None))
        .map(|p| {
            let h = p.happiness(fields.unit_power(POWER_HAPPINESS).unwrap_or(0));
            (Some(h.bucket), h.damage_percentage, h.loyalty_rate)
        })
        // No DBC: the binding's gate-failure numbers and a nil bucket.
        .unwrap_or((None, 100.0, 0.0));
    (
        has_ui,
        PetStats {
            hunter_pet: true,
            happiness,
            damage_percentage,
            loyalty_rate,
            loyalty: tables.and_then(|t| {
                t.loyalty
                    .name(u32::from(fields.unit_loyalty_level()))
                    .map(str::to_string)
            }),
            training_points: fields.unit_training_points(),
            experience: fields.unit_pet_experience(),
            family,
            icon,
            food_types,
        },
    )
}

fn feed_pet_stats(
    script: Option<NonSendMut<UiScript>>,
    bar: Res<PetBar>,
    pet: PetUnit,
    self_store: Query<&ObjectStore, With<crate::net::SelfPlayer>>,
    tables: Option<Res<PetStatTables>>,
    family_tables: Option<Res<PetFamilyTables>>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    mut last: Local<crate::ui_script::VmMemo<Option<(bool, PetStats)>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let last = last.get(&script);
    let store = pet.store(bar.spells.pet_guid);
    let family = family_for(store, &names, &commands, family_tables.as_deref());
    let fresh = stats_for(store, self_store.iter().next(), tables.as_deref(), family);
    if last.as_ref() == Some(&fresh) {
        return;
    }
    // `UNIT_HAPPINESS`, the pet frame's icon repaint (`PetFrame.lua:63`). Deviation: fired when
    // the bucket or its two tooltip figures change, not on every change of the happiness field as
    // in the reference, because no stock consumer reads the raw value.
    let happiness_moved = last.as_ref().map(|(_, s)| {
        (
            s.happiness,
            s.damage_percentage.to_bits(),
            s.loyalty_rate.to_bits(),
        )
    }) != Some((
        fresh.1.happiness,
        fresh.1.damage_percentage.to_bits(),
        fresh.1.loyalty_rate.to_bits(),
    ));
    // The pet page's other repaint events (`PetPaperDollFrame.lua:9,21`), each fired off its own
    // values so happiness drift leaves them alone; `UNIT_PET_EXPERIENCE` repaints only the XP bar.
    let xp_moved = last.as_ref().map(|(_, s)| s.experience) != Some(fresh.1.experience);
    let training_moved =
        last.as_ref().map(|(_, s)| s.training_points) != Some(fresh.1.training_points);
    if happiness_moved {
        debug!(
            "ui_pet_stats: happiness {:?} ({}% damage), loyalty {:?}",
            fresh.1.happiness, fresh.1.damage_percentage, fresh.1.loyalty
        );
    }
    // The family lands with the creature-query answer and fires nothing: the pet page registers no
    // event known to fire then (not the reference's query event, `UNIT_CLASSIFICATION_CHANGED`).
    // The page's `OnShow` covers an open; a page already open repaints on its next `UNIT_STATS`.
    if last.as_ref().map(|(_, s)| &s.family) != Some(&fresh.1.family) {
        debug!(
            "ui_pet_stats: pet family {:?}, diet {:?}",
            fresh.1.family, fresh.1.food_types
        );
    }
    *last = Some(fresh.clone());
    script.set_pet_stats(fresh.0, fresh.1);
    // Push before firing: dispatch runs the Lua handlers synchronously.
    if happiness_moved {
        // `arg1` is the unit token, as on every 1.12 `UNIT_*` event; its handlers gate on it.
        script.fire_event("UNIT_HAPPINESS", vec![ScriptValue::Str("pet".into())]);
    }
    // Fields 141/142 and 149 fire through the same token fan-out (`0x515e50`,
    // `SignalEvent2(id, "%s", token)`); `UNIT_PET_TRAINING_POINTS` reaches the page only through
    // its `arg1 == "pet"` arm (`PetPaperDollFrame.lua:45`).
    if xp_moved {
        script.fire_event("UNIT_PET_EXPERIENCE", vec![ScriptValue::Str("pet".into())]);
    }
    if training_moved {
        script.fire_event(
            "UNIT_PET_TRAINING_POINTS",
            vec![ScriptValue::Str("pet".into())],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::ObjectFields;

    const BYTES_0: u16 = 36;
    const BYTES_1: u16 = 138;
    const POWER5: u16 = 27;
    const PETNUMBER: u16 = 139;
    const PETXP: u16 = 141;
    const PETNEXTXP: u16 = 142;
    const TRAINING: u16 = 149;
    /// `OBJECT_FIELD_ENTRY`, the pet's creature-template id.
    const ENTRY: u16 = 3;
    /// vmangos `creature_template` entries: Imp (`pet_family` 23) and Stonetusk Boar (5).
    const IMP_ENTRY: u32 = 416;
    const BOAR_ENTRY: u32 = 113;

    fn hunter() -> ObjectStore {
        ObjectStore(ObjectFields::from_pairs(&[(
            BYTES_0,
            u32::from(CLASS_HUNTER) << 8,
        )]))
    }

    fn warlock() -> ObjectStore {
        ObjectStore(ObjectFields::from_pairs(&[(BYTES_0, 9 << 8)]))
    }

    /// A boar with loyalty level 6, full happiness, part-trained and mid-XP.
    fn boar() -> ObjectStore {
        ObjectStore(ObjectFields::from_pairs(&[
            (ENTRY, BOAR_ENTRY),
            (PETNUMBER, 42),
            (BYTES_1, 6 << 8),             // loyalty level in byte 1
            (POWER5, 1_000_000),           // maximum happiness
            (TRAINING, (170 << 16) | 130), // total in the high word, spent in the low
            (PETXP, 4200),
            (PETNEXTXP, 8000),
        ]))
    }

    fn chain() -> Option<benilla_formats::Chain> {
        let data = benilla_formats::wow_data_or_skip!(None);
        Some(benilla_formats::open_chain(&data).expect("open chain"))
    }

    fn tables() -> Option<PetStatTables> {
        let mut chain = chain()?;
        Some(PetStatTables {
            personalities: benilla_formats::load_pet_personalities(&mut chain)
                .expect("personality"),
            loyalty: benilla_formats::load_pet_loyalty_names(&mut chain).expect("loyalty"),
        })
    }

    fn family_tables() -> Option<PetFamilyTables> {
        let mut chain = chain()?;
        Some(PetFamilyTables {
            families: benilla_formats::load_creature_families(&mut chain).expect("families"),
            foods: benilla_formats::load_pet_food_names(&mut chain).expect("foods"),
        })
    }

    fn no_family() -> (Option<String>, Option<String>, Vec<String>) {
        (None, None, Vec::new())
    }

    /// A `NameCache` as after `SMSG_CREATURE_QUERY_RESPONSE` for `entry` landed.
    fn cache_with(entry: u32, pet_family: u32) -> NameCache {
        let mut names = NameCache::default();
        names.insert_creature(
            entry,
            Some(crate::names::CreatureRecord {
                name: "Snarl".into(),
                subname: None,
                creature_type: 1,
                pet_family,
                rank: 0,
                type_flags: 0,
                civilian: false,
                racial_leader: false,
                display_id: 0,
            }),
        );
        names
    }

    fn commands() -> (
        NetCommands,
        crossbeam_channel::Receiver<crate::net::ClientCommand>,
    ) {
        let (tx, rx) = crossbeam_channel::unbounded();
        (NetCommands(tx), rx)
    }

    /// On the real DBC data: the training-point word order, the loyalty byte, and a maxed pet's
    /// bucket with 125% damage.
    #[test]
    fn a_hunters_boar_reads_every_field() {
        let Some(t) = tables() else { return };
        let (has_ui, s) = stats_for(Some(&boar()), Some(&hunter()), Some(&t), no_family());
        assert!(has_ui && s.hunter_pet);
        assert_eq!(s.happiness, Some(3));
        assert_eq!(s.damage_percentage, 125.0);
        assert_eq!(s.loyalty_rate, 20.0);
        assert_eq!(s.loyalty.as_deref(), Some("(Loyalty Level 6) Best Friend"));
        assert_eq!(
            s.training_points,
            (170, 130),
            "TOTAL first, from the high word"
        );
        assert_eq!(s.experience, (4200, 8000));
    }

    #[test]
    fn happiness_buckets_the_raw_power() {
        let Some(t) = tables() else { return };
        let at = |raw: u32| {
            let store = ObjectStore(ObjectFields::from_pairs(&[
                (PETNUMBER, 42),
                (BYTES_1, 0),
                (POWER5, raw),
            ]));
            let (_, s) = stats_for(Some(&store), Some(&hunter()), Some(&t), no_family());
            (s.happiness, s.damage_percentage)
        };
        assert_eq!(at(0), (Some(1), 75.0), "an unhappy pet deals 75%");
        assert_eq!(at(500_000), (Some(2), 100.0));
        assert_eq!(at(1_000_000), (Some(3), 125.0));
    }

    /// A warlock's imp has a pet number, so `HasPetUI`'s first return holds, and no stats.
    #[test]
    fn a_warlocks_minion_has_a_ui_and_no_stats() {
        let Some(t) = tables() else { return };
        let (has_ui, s) = stats_for(Some(&boar()), Some(&warlock()), Some(&t), no_family());
        assert!(
            has_ui,
            "the pet number is what HasPetUI's first return reads"
        );
        assert!(!s.hunter_pet);
        assert_eq!(s.happiness, None);
        assert_eq!(s.loyalty, None);
        assert_eq!(s.training_points, (0, 0));
        assert_eq!(s.experience, (0, 0));
    }

    /// `PetHasActionBar` is the cached guid alone; the paper doll also needs a pet number.
    #[test]
    fn no_pet_number_means_no_pet_ui() {
        let Some(t) = tables() else { return };
        let possessed = ObjectStore(ObjectFields::from_pairs(&[(POWER5, 1_000_000)]));
        let (has_ui, s) = stats_for(Some(&possessed), Some(&hunter()), Some(&t), no_family());
        assert!(!has_ui);
        assert!(!s.hunter_pet);
    }

    /// Level 0 is nil, not "Rebellious": the client's own bound.
    #[test]
    fn loyalty_level_zero_is_nil() {
        let Some(t) = tables() else { return };
        let fresh = ObjectStore(ObjectFields::from_pairs(&[
            (PETNUMBER, 42),
            (BYTES_1, 0),
            (POWER5, 1_000_000),
        ]));
        let (_, s) = stats_for(Some(&fresh), Some(&hunter()), Some(&t), no_family());
        assert_eq!(s.loyalty, None);
        assert_eq!(s.happiness, Some(3), "…but happiness still answers");
    }

    #[test]
    fn absent_dbc_data_degrades_to_the_failure_numbers() {
        let (has_ui, s) = stats_for(Some(&boar()), Some(&hunter()), None, no_family());
        assert!(has_ui && s.hunter_pet);
        assert_eq!(s.happiness, None);
        assert_eq!((s.damage_percentage, s.loyalty_rate), (100.0, 0.0));
        // The descriptor values need no table.
        assert_eq!(s.training_points, (170, 130));
    }

    /// A missing tooltip key is silent: `getglobal` answers nil and the tooltip loses a line.
    #[test]
    fn every_happiness_string_resolves_in_the_real_global_strings() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let s = benilla_ui::script::UiScript::new().expect("VM");
        s.run(&String::from_utf8_lossy(&src)).expect("runs clean");
        let g = |key: &str| {
            s.lua()
                .globals()
                .get::<String>(key)
                .ok()
                .unwrap_or_default()
        };

        // The bucket names are built by concatenation (`"PET_HAPPINESS"..happiness`).
        assert_eq!(g("PET_HAPPINESS1"), "Unhappy");
        assert_eq!(g("PET_HAPPINESS2"), "Content");
        assert_eq!(g("PET_HAPPINESS3"), "Happy");
        assert!(
            g("PET_DAMAGE_PERCENTAGE").contains("%d"),
            "the damage line formats the percentage in: {:?}",
            g("PET_DAMAGE_PERCENTAGE")
        );
        assert!(!g("LOSING_LOYALTY").is_empty());
        assert!(!g("GAINING_LOYALTY").is_empty());
    }

    #[test]
    fn no_pet_is_no_ui() {
        let (has_ui, s) = stats_for(None, Some(&hunter()), None, no_family());
        assert!(!has_ui);
        assert_eq!(s, PetStats::default());
    }

    /// On the real DBC data, through the binding's three nil sources; asking by the guid's entry
    /// slot, a pet number, would answer nil forever and silently.
    #[test]
    fn the_pets_family_resolves_off_its_descriptor_entry() {
        let Some(t) = family_tables() else { return };
        let (cmds, rx) = commands();

        // 1. No pet.
        let names = NameCache::default();
        assert_eq!(family_for(None, &names, &cmds, Some(&t)), no_family());
        assert!(rx.try_recv().is_err(), "nothing to ask about");

        // 2. Unanswered: nil, and the ask goes out once.
        let pet = ObjectStore(ObjectFields::from_pairs(&[
            (ENTRY, IMP_ENTRY),
            (PETNUMBER, 7),
        ]));
        assert_eq!(
            family_for(Some(&pet), &names, &cmds, Some(&t)),
            no_family(),
            "un-queried is nil, not a guess"
        );
        assert!(
            matches!(
                rx.try_recv(),
                Ok(crate::net::ClientCommand::CreatureQuery { entry, .. }) if entry == IMP_ENTRY
            ),
            "the pet's DESCRIPTOR entry is what gets queried"
        );
        assert!(rx.try_recv().is_err(), "ask-once");

        // 3. Family 0, a template with no family: nil.
        let names = cache_with(IMP_ENTRY, 0);
        assert_eq!(family_for(Some(&pet), &names, &cmds, Some(&t)), no_family());

        // 4. The Imp's family 23: a word and an empty diet, food mask 0 in the shipped DBC.
        let names = cache_with(IMP_ENTRY, 23);
        assert_eq!(
            family_for(Some(&pet), &names, &cmds, Some(&t)),
            (
                Some("Imp".into()),
                // Rows 15 (Felhunter) and 23 (Imp) ship this placeholder; the stable, the
                // column's only reader, never shows it for a warlock.
                Some("Interface\\Icons\\Ability_Druid_CatForm".into()),
                Vec::new()
            )
        );

        // 5. A boar (family 5): the word and six diets, in bit order.
        let names = cache_with(BOAR_ENTRY, 5);
        let (name, icon, diet) = family_for(Some(&boar()), &names, &cmds, Some(&t));
        assert_eq!(name.as_deref(), Some("Boar"));
        assert_eq!(
            icon.as_deref(),
            Some("Interface\\Icons\\Ability_Hunter_Pet_Boar")
        );
        assert_eq!(diet, ["Meat", "Fish", "Cheese", "Bread", "Fungus", "Fruit"]);

        // 6. No DBC tables: nil.
        let names = cache_with(BOAR_ENTRY, 5);
        assert_eq!(family_for(Some(&boar()), &names, &cmds, None), no_family());
    }

    /// A boar under a non-hunter (family 5, food mask 63) is the one case that shows the split, as
    /// every warlock minion family ships mask 0.
    #[test]
    fn a_charmed_beast_keeps_its_family_word_and_loses_its_diet() {
        let Some(t) = tables() else { return };
        let boar_diet: Vec<String> = ["Meat", "Fish", "Cheese", "Bread", "Fungus", "Fruit"]
            .map(String::from)
            .to_vec();
        let (_, s) = stats_for(
            Some(&boar()),
            Some(&warlock()),
            Some(&t),
            (Some("Boar".into()), None, boar_diet.clone()),
        );
        assert!(!s.hunter_pet);
        assert_eq!(s.family.as_deref(), Some("Boar"), "the word is ungated");
        assert!(
            s.food_types.is_empty(),
            "…but the diet shares 0x6116e0 with the stats"
        );
        assert_eq!(s.loyalty, None, "…as does the hunter machinery");
        assert_eq!(s.happiness, None);

        // Under a hunter it gets both.
        let (_, s) = stats_for(
            Some(&boar()),
            Some(&hunter()),
            Some(&t),
            (Some("Boar".into()), None, boar_diet.clone()),
        );
        assert_eq!(s.family.as_deref(), Some("Boar"));
        assert_eq!(s.food_types, boar_diet);
    }

    #[test]
    fn no_pet_drops_the_family_too() {
        let (_, s) = stats_for(
            None,
            Some(&hunter()),
            None,
            (Some("Imp".into()), None, vec![]),
        );
        assert_eq!(s.family, None);
        assert!(s.food_types.is_empty());
    }

    /// `UNIT_PET` must reach Lua with `HasPetUI()` already answering, as `PetTab_Update` reads it;
    /// only a real schedule orders the feeds, so this runs one `app.update()` on a cold pet.
    #[test]
    fn unit_pet_reaches_lua_with_has_pet_ui_already_true() {
        use crate::char_select::ClientState;
        use crate::net::GuidIndex;
        use crate::ui_pet::UiPetPlugin;

        const PET_GUID: u64 = 0xf140_0000_0000_002a;

        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .insert_state(ClientState::InWorld)
            .init_resource::<NameCache>()
            .init_resource::<GuidIndex>()
            .init_resource::<crate::target::Selection>()
            .init_resource::<crate::net::SelfGuid>()
            .init_resource::<crate::ui_script::UiClock>()
            .init_resource::<crate::ui_action::UiErrorKeys>()
            .init_resource::<crate::net::Reputations>()
            .init_resource::<crate::spell::QueuedMeleeSpell>()
            .init_resource::<crate::spell::AutoRepeatActive>()
            .init_resource::<crate::ui_loot::LootState>()
            .init_resource::<crate::ui_loot::LootLatch>()
            .add_message::<crate::creature_anim::SheathRequest>()
            .add_message::<crate::net::FieldChanged>()
            .insert_resource(NetCommands(tx))
            .add_plugins((UiPetStatsPlugin, UiPetPlugin));

        // The pet is in the world when the bar's guid lands: a cold summon.
        let pet = app.world_mut().spawn(boar()).id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(PET_GUID, pet);
        app.world_mut().resource_mut::<PetBar>().spells.pet_guid = PET_GUID;

        // A frame standing in for the pet page records what `HasPetUI()` answers at dispatch.
        let script = UiScript::new().unwrap();
        script
            .run(
                r#"
                BENILLA_SAW = "never fired"
                local f = CreateFrame("Frame")
                f:RegisterEvent("UNIT_PET")
                f:SetScript("OnEvent", function()
                    BENILLA_SAW = HasPetUI() and "has pet UI" or "no pet UI"
                end)
                "#,
            )
            .unwrap();
        app.insert_non_send_resource(script);

        app.update();

        let saw: String = app
            .world_mut()
            .non_send_resource::<UiScript>()
            .eval("return BENILLA_SAW")
            .unwrap();
        assert_eq!(
            saw, "has pet UI",
            "UNIT_PET must not reach Lua ahead of the snapshot its handlers read — unordered, \
             this answered \"no pet UI\" and the Pet tab stayed down for the session"
        );
    }
}
