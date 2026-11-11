//! Hero Defense: nazvy, ikony a popisy kouzel, kterymi hrac stavi veze.
//!
//! Server stavi veze pres existujici NPC kouzla klienta (mireni na zem, bez ceny), ktera si
//! v `spell_template` prepise na vyvolani veze (`sql/hero_defense/hero_defense_towers.sql`).
//! Klient by u nich ukazal puvodni nazev a ikonu (Meteor, Blizzard...), proto je tu prepisuje.
//! Cisla kouzel musi sedet s `HD_TOWERS` v serveru (`src/scripts/custom/hd_defs.h`).

use std::collections::HashMap;

use super::SpellDisplay;

/// `SpellRange.dbc` radek 13: 0 az 50 000 yd (Eye of Kilrogg).
const HD_RANGE_ANYWHERE: u32 = 13;

/// (kouzlo, nazev veze, rasa, ikona)
const TOWER_SPELLS: [(u32, &str, &str, &str); 9] = [
    (8333, "Human Cannon Bastion", "Human", "INV_Misc_Bomb_05"),
    (25823, "Human Holy Light Spire", "Human", "Spell_Holy_HolyBolt"),
    (13808, "Human Watchtower", "Human", "Ability_TownWatch"),
    (26540, "Night Elf Glaive", "Night Elf", "Ability_UpgradeMoonGlaive"),
    (25019, "Night Elf Moonwell", "Night Elf", "Spell_Nature_MoonGlow"),
    (26462, "Night Elf Sentinel", "Night Elf", "Ability_EyeOfTheOwl"),
    (23105, "Orc Shamanic Totem", "Orc", "Spell_Nature_StoneClawTotem"),
    (24340, "Orc Siege Catapult", "Orc", "INV_Ammo_FireTar"),
    (19717, "Orc Watchtower", "Orc", "INV_Misc_Spyglass_01"),
];

pub(super) fn apply(spells: &mut HashMap<u32, SpellDisplay>) {
    for (id, name, race, icon) in TOWER_SPELLS {
        let Some(spell) = spells.get_mut(&id) else {
            continue;
        };
        spell.name = name.to_string();
        spell.rank = Some(race.to_string());
        spell.icon = Some(format!("Interface\\Icons\\{icon}"));
        spell.description = Some(format!("Postavi vez {name} na pole mrizky areny."));
        // Stavba odkudkoli, i z "ocicka" (spectate) daleko od hrdiny: SpellRange 13 "Anywhere".
        // Server ma stejny dosah v hero_defense_towers.sql.
        spell.range_index = HD_RANGE_ANYWHERE;
    }
}
