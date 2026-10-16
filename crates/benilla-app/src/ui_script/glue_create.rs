//! What the stock create screen's verbs read off the DBCs ([`GlueCreateCatalog`]), assembled from
//! the host's catalogs the way the glue build walks them (`0x470430`) and the verbs look them up.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_formats::{CharCreateCatalog, ChrClasses, FactionCatalog, FactionTemplate};
use benilla_ui::script::{
    GlueCreateCatalog, GlueCreateClass, GlueCreateCounts, GlueCreateRace, GlueNameRow,
};

/// The catalogs the create screen's verbs and the roster's strings read.
#[derive(SystemParam)]
pub(crate) struct CreateSources<'w> {
    pub(crate) create: Option<Res<'w, crate::entities::CharCreate>>,
    pub(crate) classes: Option<Res<'w, crate::chr_classes::ChrClassTable>>,
    pub(crate) factions: Option<Res<'w, crate::target::Factions>>,
    pub(crate) creatures: Option<Res<'w, crate::entities::Creatures>>,
}

impl CreateSources<'_> {
    /// One of the catalogs arrived or changed since the system last ran.
    pub(crate) fn changed(&self) -> bool {
        self.create.as_ref().is_some_and(|r| r.is_changed())
            || self.classes.as_ref().is_some_and(|r| r.is_changed())
            || self.factions.as_ref().is_some_and(|r| r.is_changed())
            || self.creatures.as_ref().is_some_and(|r| r.is_changed())
    }

    /// Every catalog has arrived, so the verbs' catalog is whole: a create screen shown before
    /// then lists races with no faction, whose `OnShow` raises.
    pub(crate) fn complete(&self) -> bool {
        self.create.is_some()
            && self.classes.is_some()
            && self.factions.is_some()
            && self.creatures.is_some()
    }

    /// The verbs' catalog, empty without `ChrRaces`.
    pub(crate) fn catalog(&self) -> GlueCreateCatalog {
        match &self.create {
            Some(create) => create_catalog(
                &create.0,
                self.classes.as_deref().map(|c| &c.0),
                self.factions.as_deref().map(|f| f.catalog()),
                self.creatures.as_deref(),
            ),
            None => GlueCreateCatalog::default(),
        }
    }
}

/// The verbs' catalog off the world's resources, for the glue build that seats a VM.
pub(crate) fn world_catalog(world: &World) -> GlueCreateCatalog {
    match world.get_resource::<crate::entities::CharCreate>() {
        Some(create) => create_catalog(
            &create.0,
            world
                .get_resource::<crate::chr_classes::ChrClassTable>()
                .map(|c| &c.0),
            world
                .get_resource::<crate::target::Factions>()
                .map(|f| f.catalog()),
            world.get_resource::<crate::entities::Creatures>(),
        ),
        None => GlueCreateCatalog::default(),
    }
}

/// The `FactionGroup` rows, in file order, with a nonzero bit index that `template`'s group mask
/// holds (`0x4714b0`-`0x4714cd`, `0x4704d0`-`0x47050d`); the shift takes the index mod 32, as
/// `shl ebx,cl` does.
fn groups<'a>(
    factions: &'a FactionCatalog,
    template: &'a FactionTemplate,
) -> impl Iterator<Item = &'a benilla_formats::FactionGroupRow> {
    factions
        .faction_groups()
        .iter()
        .filter(|g| g.mask_id != 0 && template.group_mask & 1u32.wrapping_shl(g.mask_id) != 0)
}

/// Assemble the verbs' catalog:
///
/// - the race list `0x470430` builds, `ChrRaces` in file order less rows flagged `0x1`, first the
///   races with a faction group whose `InternalName` is `Alliance`, then `Horde` (case-folded,
///   `0x64a4c0`), each row's name, file string, first faction group and tokens;
/// - each listed race's class list (`0x4706b0`) and the `ChrClasses` rows they name;
/// - each listed race's five counts by sex, and which of its three sexes has a body with a model
///   (`0x5ec480`: the race row's display for the sex, then its model rows);
/// - `NameGen` in file order.
pub(crate) fn create_catalog(
    create: &CharCreateCatalog,
    classes: Option<&ChrClasses>,
    factions: Option<&FactionCatalog>,
    creatures: Option<&crate::entities::Creatures>,
) -> GlueCreateCatalog {
    let mut catalog = GlueCreateCatalog::default();
    let Some(factions) = factions else {
        return catalog;
    };
    for side in ["Alliance", "Horde"] {
        for row in create.race_rows() {
            if row.flags & 0x1 != 0 {
                continue;
            }
            let Some(template) = factions.template(row.faction) else {
                continue;
            };
            for _ in groups(factions, template).filter(|g| g.internal.eq_ignore_ascii_case(side)) {
                let id = row.id;
                let token = |t: Option<&str>| t.unwrap_or_default().to_string();
                catalog.races.push(GlueCreateRace {
                    id,
                    name: token(create.race_name(id)),
                    file: token(create.race_file(id)),
                    faction: groups(factions, template)
                        .next()
                        .map(|g| (g.name.clone(), g.internal.clone())),
                    tokens: [
                        token(create.facial_hair_customization(id, 0)),
                        token(create.facial_hair_customization(id, 1)),
                        token(create.hair_customization(id)),
                    ],
                });
            }
        }
    }
    for race in catalog.races.iter().map(|r| r.id).collect::<Vec<_>>() {
        let list = create.class_list(race);
        for &class in &list {
            if let Some(table) = classes.filter(|t| t.has_row(u32::from(class))) {
                catalog.classes.insert(
                    class,
                    GlueCreateClass {
                        name: table.name(u32::from(class)).unwrap_or_default().into(),
                        file: table.file(u32::from(class)).unwrap_or_default().into(),
                    },
                );
            }
        }
        catalog.class_lists.insert(race, list);
        for sex in 0..=2u8 {
            if let Some(r) = create.ranges(race, sex) {
                catalog.counts.insert(
                    (race, sex),
                    GlueCreateCounts {
                        skin: i32::from(r.skin),
                        face: i32::from(r.face),
                        hair_styles: i32::from(r.hair_style),
                        hair_colors: (0..r.hair_style)
                            .map(|style| i32::from(create.hair_colors(race, sex, style)))
                            .collect(),
                        facial_hair: i32::from(r.facial_hair),
                    },
                );
            }
            let model = create
                .body_display(race, sex)
                .is_some_and(|d| creatures.is_some_and(|c| c.resolves_model(d)));
            if model {
                catalog.models.insert((race, sex));
            }
        }
    }
    catalog.names = create
        .name_gen()
        .iter()
        .map(|r| GlueNameRow {
            race: r.race,
            sex: r.sex,
            name: r.name.clone(),
        })
        .collect();
    catalog
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Off the player's own DBCs: the eight races, Alliance then Horde in row order with Goblin
    /// off the list, their faction groups, tokens, class lists and models.
    #[test]
    fn the_catalog_is_the_shipped_rows() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let create = CharCreateCatalog::load(&mut chain).expect("ChrRaces");
        let classes = benilla_formats::load_chr_classes(&mut chain).expect("ChrClasses");
        let factions = benilla_formats::load_faction_catalog(&mut chain).expect("factions");
        let creatures = crate::entities::Creatures::from_catalog(
            benilla_formats::load_creature_catalog(&mut chain).expect("creatures"),
        );
        let c = create_catalog(&create, Some(&classes), Some(&factions), Some(&creatures));
        let races: Vec<(u8, &str, &str, &str)> = c
            .races
            .iter()
            .map(|r| {
                (
                    r.id,
                    r.name.as_str(),
                    r.file.as_str(),
                    r.faction.as_ref().map_or("", |f| f.1.as_str()),
                )
            })
            .collect();
        assert_eq!(
            races,
            [
                (1, "Human", "Human", "Alliance"),
                (3, "Dwarf", "Dwarf", "Alliance"),
                (4, "Night Elf", "NightElf", "Alliance"),
                (7, "Gnome", "Gnome", "Alliance"),
                (2, "Orc", "Orc", "Horde"),
                (5, "Undead", "Scourge", "Horde"),
                (6, "Tauren", "Tauren", "Horde"),
                (8, "Troll", "Troll", "Horde"),
            ]
        );
        let tauren = &c.races[6];
        assert_eq!(tauren.tokens, ["NORMAL", "HAIR", "HORNS"].map(String::from));
        assert_eq!(
            tauren.faction,
            Some(("Horde".to_string(), "Horde".to_string()))
        );
        assert_eq!(c.class_lists[&3], [1, 2, 3, 4, 5]);
        assert_eq!(
            c.classes[&11],
            GlueCreateClass {
                name: "Druid".into(),
                file: "DRUID".into()
            }
        );
        for r in &c.races {
            assert!(c.models.contains(&(r.id, 0)) && c.models.contains(&(r.id, 1)));
            assert!(!c.models.contains(&(r.id, 2)), "no body for sex 2");
            let male = &c.counts[&(r.id, 0)];
            assert!(male.skin > 0 && male.face > 0 && male.facial_hair > 0);
            assert_eq!(male.hair_colors.len(), male.hair_styles as usize);
        }
        assert_eq!(c.names.len(), 1405);
        // #1073: Orc female's eight hair styles, each with its colours.
        let orc_female = &c.counts[&(2, 1)];
        assert_eq!(orc_female.hair_styles, 8);
        assert_eq!(orc_female.hair_colors, [8; 8]);
        // Without the faction tables the walk finds no race.
        assert!(
            create_catalog(&create, Some(&classes), None, Some(&creatures))
                .races
                .is_empty()
        );
    }
}
