//! Every loader of a localized DBC string reads the chain's locale column: at the install's own
//! locale each one finds strings, and at a locale the install does not fill each finds none. A
//! loader that read a fixed column would find the same strings at both.

use crate::{Chain, EmoteLine, Locale};

/// The localized strings one loader yields, empties dropped.
type Probe = fn(&mut Chain) -> Vec<String>;

fn some(strings: impl IntoIterator<Item = Option<String>>) -> Vec<String> {
    strings
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect()
}

fn owned<'a>(strings: impl IntoIterator<Item = Option<&'a str>>) -> Vec<String> {
    some(strings.into_iter().map(|s| s.map(str::to_string)))
}

/// One probe per converted loader and column.
const PROBES: &[(&str, Probe)] = &[
    ("AreaPOI.Name", |c| {
        let cat = crate::load_area_poi_catalog(c).unwrap();
        some(cat.rows().map(|(_, p)| Some(p.name.clone())))
    }),
    ("AreaPOI.Description", |c| {
        let cat = crate::load_area_poi_catalog(c).unwrap();
        some(cat.rows().map(|(_, p)| Some(p.description.clone())))
    }),
    ("AreaTable.AreaName", |c| {
        let cat = crate::load_area_table_catalog(c).unwrap();
        some(cat.rows().map(|r| Some(r.name.clone())))
    }),
    ("AreaTable.AreaName (area sound)", |c| {
        let cat = crate::load_area_sound_catalog(c).unwrap();
        owned([cat.resolve(12).map(|a| a.area_name)])
    }),
    ("AreaTable.AreaName, QuestSort.Name (quest headers)", |c| {
        let names = crate::load_quest_header_names(c).unwrap();
        owned([names.resolve(12), names.resolve(-1)])
    }),
    ("QuestInfo.Name", |c| {
        let names = crate::load_quest_tag_names(c).unwrap();
        owned((0..100).map(|id| names.resolve(id)))
    }),
    ("AuctionHouse.Name", |c| {
        let cat = crate::load_auction_houses(c).unwrap();
        owned((0..10).map(|id| cat.get(id).map(|h| h.name.as_str())))
    }),
    ("Cfg_Categories.Name", |c| {
        let region = crate::client_region(c);
        let rows = crate::load_realm_categories(c, region).unwrap();
        some(rows.into_iter().map(|r| Some(r.name)))
    }),
    ("ChatChannels.Name, Shortcut", |c| {
        let cat = crate::load_chat_channels_catalog(c).unwrap();
        some(
            cat.rows()
                .iter()
                .flat_map(|r| [Some(r.pattern.clone()), Some(r.shortcut.clone())]),
        )
    }),
    ("ChrClasses.Name", |c| {
        let cat = crate::load_chr_classes(c).unwrap();
        owned((1..12).map(|id| cat.name(id)))
    }),
    ("ChrRaces.Name", |c| {
        let cat = crate::CharCreateCatalog::load(c).unwrap();
        owned((1..9).map(|race| cat.race_name(race)))
    }),
    ("CreatureFamily.Name", |c| {
        let cat = crate::load_creature_families(c).unwrap();
        owned((0..40).map(|id| cat.name(id)))
    }),
    ("ItemPetFood.Name", |c| {
        let cat = crate::load_pet_food_names(c).unwrap();
        owned((0..10).map(|id| cat.name(id)))
    }),
    ("EmotesTextData.Text", |c| {
        let cat = crate::load_emote_text_catalog(c).unwrap();
        let line = EmoteLine {
            performer: "Bob",
            performer_is_you: false,
            performer_female: false,
            target: "Jane",
            your_name: "Me",
        };
        some([cat.compose(101, &line)])
    }),
    ("Exhaustion.Name", |c| {
        let rows = crate::load_exhaustion(c).unwrap();
        some(rows.into_iter().map(|r| Some(r.name)))
    }),
    ("Faction.Name, Description; FactionGroup.Name", |c| {
        let cat = crate::load_faction_catalog(c).unwrap();
        let ids: Vec<u32> = cat.reputation_factions().map(|(id, _)| id).collect();
        let mut out = owned(ids.iter().map(|&id| cat.faction_name(id)));
        out.extend(owned(ids.iter().map(|&id| cat.faction_description(id))));
        out.extend(some(
            cat.faction_groups().iter().map(|g| Some(g.name.clone())),
        ));
        out
    }),
    ("GameTips.Text", |c| {
        let cat = crate::load_game_tips(c).unwrap();
        owned((0..cat.len()).map(|i| cat.get(i)))
    }),
    ("GMSurveyQuestions.Text", |c| {
        let survey = crate::load_gm_survey(c).unwrap();
        some(survey.texts.into_iter().map(|(_, t)| Some(t)))
    }),
    ("GMTicketCategory.Name", |c| {
        let cat = crate::load_gm_ticket_categories(c).unwrap();
        some(cat.categories().iter().map(|g| Some(g.name.clone())))
    }),
    ("ItemRandomProperties.Suffix", |c| {
        let cat = crate::load_random_property_catalog(c).unwrap();
        some(cat.iter().map(|(_, p)| Some(p.suffix.clone())))
    }),
    ("ItemSet.Name", |c| {
        let cat = crate::load_item_sets(c).unwrap();
        owned((0..1000).map(|id| cat.set(id).map(|s| s.name.as_str())))
    }),
    (
        "ItemSubClass.DisplayName, VerboseName; ItemSubClassMask.Name",
        |c| {
            let cat = crate::load_item_sub_classes(c).unwrap();
            let mut out = owned((0..21).map(|sub| cat.name(2, sub)));
            out.extend(owned((0..21).map(|sub| cat.display_name(2, sub))));
            out.extend(some([cat.requirement_name(2, 0x0001_0000 | 0x0002_0000)]));
            out
        },
    ),
    ("ItemBagFamily.Name", |c| {
        let cat = crate::load_item_bag_families(c).unwrap();
        owned((0..10).map(|id| cat.name(id)))
    }),
    ("ItemClass.Name", |c| {
        let cat = crate::load_item_classes(c).unwrap();
        owned((0..16).map(|id| cat.name(id)))
    }),
    ("Languages.Name", |c| {
        let langs = crate::load_languages(c).unwrap();
        langs.names().map(|(_, n)| n.to_string()).collect()
    }),
    ("Languages.Name (default languages)", |c| {
        let langs = crate::load_default_languages(c).unwrap();
        owned((1..9).map(|race| langs.name(race)))
    }),
    ("LFGDungeons.Name", |c| {
        let rows = crate::load_lfg_dungeons(c).unwrap();
        some(rows.into_iter().map(|r| Some(r.name)))
    }),
    ("LockType.Name", |c| {
        let cat = crate::load_lock_type_catalog(c).unwrap();
        owned((0..20).map(|id| cat.name(id)))
    }),
    ("Map.MapName, MapDescription0, MapDescription1", |c| {
        let cat = crate::load_map_catalog(c).unwrap();
        let mut out = owned((0..600).map(|id| cat.name(id)));
        for id in 0..600 {
            if let Some(bg) = cat.battleground(id) {
                out.extend(some(bg.descriptions.iter().cloned().map(Some)));
            }
        }
        out
    }),
    ("Package.Name", |c| {
        let rows = crate::load_packages(c).unwrap();
        some(rows.into_iter().map(|r| Some(r.name)))
    }),
    ("PetLoyalty.Name", |c| {
        let cat = crate::load_pet_loyalty_names(c).unwrap();
        owned((0..10).map(|id| cat.name(id)))
    }),
    ("ServerMessages.Text", |c| {
        let cat = crate::load_server_messages_catalog(c).unwrap();
        owned((0..10).map(|id| cat.text(id)))
    }),
    ("SkillLine.Name, Description; SkillLineCategory.Name", |c| {
        let cat = crate::load_skill_line_catalog(c).unwrap();
        let mut out = Vec::new();
        for id in 0..800 {
            if let Some(line) = cat.line(id) {
                out.extend(some([
                    Some(line.name.clone()),
                    Some(line.description.clone()),
                ]));
            }
        }
        out.extend(owned((0..12).map(|id| cat.category(id).map(|(n, _)| n))));
        out
    }),
    (
        "Spell.Name, NameSubtext, Description, AuraDescription",
        |c| {
            let cat = crate::load_spell_catalog(c).unwrap();
            let mut out = Vec::new();
            // Arcane Intellect, Rank 1: all four strings.
            if let Some(s) = cat.get(1459) {
                out.extend(some([
                    Some(s.name.clone()),
                    s.rank.clone(),
                    s.description.clone(),
                    s.aura_description.clone(),
                ]));
            }
            out
        },
    ),
    ("SpellDispelType.Name", |c| {
        let cat = crate::load_spell_dispel_types(c).unwrap();
        owned((0..10).map(|id| cat.name(id)))
    }),
    ("SpellFocusObject.Name", |c| {
        let cat = crate::load_spell_focus_catalog(c).unwrap();
        owned((0..200).map(|id| cat.name(id)))
    }),
    ("SpellMechanic.Name", |c| {
        let cat = crate::load_spell_mechanic_catalog(c).unwrap();
        owned((0..30).map(|id| cat.name(id)))
    }),
    ("SpellShapeshiftForm.Name", |c| {
        let forms = crate::load_shapeshift_forms(c).unwrap();
        some(forms.into_values().map(|f| Some(f.name)))
    }),
    ("SpellItemEnchantment.Name", |c| {
        let cat = crate::load_enchant_catalog(c).unwrap();
        owned((0..2000).map(|id| cat.name(id)))
    }),
    ("TalentTab.Name", |c| {
        let cat = crate::load_talent_catalog(c).unwrap();
        some(
            cat.tabs_for_class(1, 1)
                .into_iter()
                .map(|t| Some(t.name.clone())),
        )
    }),
    ("TaxiNodes.Name", |c| {
        let cat = crate::load_taxi_nodes(c).unwrap();
        some(cat.rows().map(|n| Some(n.name.clone())))
    }),
    ("WMOAreaTable.AreaName", |c| {
        let cat = crate::load_wmo_area_catalog(c).unwrap();
        some([cat
            .resolve(53, 2, 9999)
            .or_else(|| cat.resolve(53, 0, 9999))
            .map(|a| a.name)])
    }),
    ("WorldStateUI.Text, Tooltip, DynamicTooltip", |c| {
        let cat = crate::load_world_state_ui_catalog(c).unwrap();
        some(cat.rows().flat_map(|(_, r)| {
            [
                Some(r.text.clone()),
                Some(r.tooltip.clone()),
                Some(r.dynamic_tooltip.clone()),
            ]
        }))
    }),
];

#[test]
fn every_localized_loader_reads_the_chains_locale() {
    let data = crate::wow_data_or_skip!();
    let mut own = crate::open_chain(&data).expect("open chain");
    let index = own.locale().index();
    let other = Locale::from_index((index + 1) % 8).unwrap();
    let mut other_chain = crate::open_chain(&data)
        .expect("open chain")
        .with_locale(other);
    let mut failures = Vec::new();
    for (what, probe) in PROBES {
        let at_own = probe(&mut own);
        let at_other = probe(&mut other_chain);
        if at_own.is_empty() {
            failures.push(format!("{what}: nothing at {}", own.locale().name()));
        }
        if !at_other.is_empty() {
            failures.push(format!(
                "{what}: {} strings at {}, first {:?}",
                at_other.len(),
                other.name(),
                at_other[0]
            ));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
