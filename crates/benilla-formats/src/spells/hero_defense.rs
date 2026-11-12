//! Hero Defense: nazvy, ikony a popisy kouzel, kterymi hrac stavi veze.
//!
//! Server stavi veze pres existujici NPC kouzla klienta (mireni na zem, bez ceny), ktera si
//! v `spell_template` prepise na vyvolani veze (`sql/hero_defense/hero_defense_towers.sql`).
//! Klient by u nich ukazal puvodni nazev a ikonu (Meteor, Blizzard...), proto je tu prepisuje.
//! Cisla kouzel musi sedet s `HD_LINES` v serveru (`src/scripts/custom/hd_defs.h`): 3 rasy po
//! 7 liniich vezi, kouzlo stavi zakladni vez linie (vylepseni jsou v panelu veze v addonu).

use std::collections::HashMap;

use super::SpellDisplay;

/// `SpellRange.dbc` radek 13: 0 az 50 000 yd (Eye of Kilrogg).
const HD_RANGE_ANYWHERE: u32 = 13;

/// (kouzlo, nazev zakladni veze, rasa, ikona); poradi jako `HD_LINES` v serveru.
const TOWER_SPELLS: [(u32, &str, &str, &str); 21] = [
    (13808, "Guard Tower", "Human", "Ability_TownWatch"),
    (25823, "Spire", "Human", "Spell_Holy_HolyBolt"),
    (8333, "Cannon Bastion", "Human", "INV_Misc_Bomb_05"),
    (4071, "Barracks", "Human", "Ability_Warrior_DefensiveStance"),
    (4072, "Lightning Tower", "Human", "Spell_Nature_LightningBolt"),
    (19805, "Ice Tower", "Human", "Spell_Frost_FrostNova"),
    (4100, "Arcane Tower", "Human", "Spell_Arcane_StarFire"),
    (26462, "Sentinel Tower", "Night Elf", "Ability_EyeOfTheOwl"),
    (25019, "Moonwell", "Night Elf", "Spell_Nature_MoonGlow"),
    (26540, "Glaive Thrower", "Night Elf", "Ability_UpgradeMoonGlaive"),
    (21688, "Starwatch Tower", "Night Elf", "Spell_Nature_StarFall"),
    (22700, "Treant Tower", "Night Elf", "Spell_Nature_ForceOfNature"),
    (27661, "Chimaera Tower", "Night Elf", "Ability_Hunter_Pet_WindSerpent"),
    (15252, "Cenarius Tower", "Night Elf", "Spell_Nature_Regeneration"),
    (19717, "Watch Tower", "Orc", "INV_Misc_Spyglass_01"),
    (23105, "Totem", "Orc", "Spell_Nature_StoneClawTotem"),
    (24340, "Siege Catapult", "Orc", "INV_Ammo_FireTar"),
    (3720, "Spirit Tower", "Orc", "Spell_Nature_SpiritWolf"),
    (3823, "Serpent Tower", "Orc", "Spell_Nature_GuardianWard"),
    (23056, "Ironhoof Tower", "Orc", "Ability_Mount_Kodo_01"),
    (897, "Wyvern Tower", "Orc", "INV_Spear_02"),
];

pub(super) fn apply(spells: &mut HashMap<u32, SpellDisplay>) {
    for (id, name, race, icon) in TOWER_SPELLS {
        let Some(spell) = spells.get_mut(&id) else {
            continue;
        };
        spell.name = name.to_string();
        spell.rank = Some(race.to_string());
        spell.icon = Some(format!("Interface\\Icons\\{icon}"));
        spell.description = Some(format!(
            "Postavi vez {name} na pole mrizky areny. Vylepseni a prodej: vez vyber jako cil."
        ));
        // Stavba odkudkoli, i z "ocicka" (spectate) daleko od hrdiny: SpellRange 13 "Anywhere".
        // Server ma stejny dosah v hero_defense_towers.sql.
        spell.range_index = HD_RANGE_ANYWHERE;
    }

    // Svitek zkusenosti (item 90200 z truhly po vlne): kouzlo-nosic 5202 (MHTest01), server
    // ho prepise v hero_defense_loot.sql. Text je radek "Use:" v tooltipu svitku.
    if let Some(spell) = spells.get_mut(&XP_SCROLL_SPELL) {
        spell.name = "Svitek zkusenosti".to_string();
        spell.rank = None;
        spell.icon = Some("Interface\\Icons\\INV_Scroll_03".to_string());
        spell.description = Some("Prida 25 % zkusenosti potrebnych do dalsiho levelu.".to_string());
    }

    // Strely vezi, ktere jsou v klientu moc rychle. Musi sedet se serverem
    // (hero_defense_towers.sql, cast "rychlost strel"): server podle ni pocita, kdy zasah prijde.
    for (id, speed) in HD_SPELL_SPEEDS {
        if let Some(spell) = spells.get_mut(&id) {
            spell.speed = speed;
        }
    }

    // Vizualni aury vezi (debuffy na neprateli, buffy vezi): server je prepisuje na neskodnou
    // auru (hero_defense_towers.sql, cast "vizualni aury"), tady dostanou jmeno, ikonu a text.
    for (id, name, icon, text) in HD_VISUAL_AURAS {
        if let Some(spell) = spells.get_mut(&id) {
            spell.name = name.to_string();
            spell.rank = None;
            spell.icon = Some(format!("Interface\\Icons\\{icon}"));
            spell.description = Some(text.to_string());
            spell.aura_description = Some(text.to_string());
        }
    }

    // Melee ma manu misto rage: jeho (zatim zastupna) kouzla Warriora stoji manu. Musi sedet
    // se serverem (hero_defense_heroes.sql: mana = rage x 2).
    for (id, mana) in HD_MANA_SPELLS {
        if let Some(spell) = spells.get_mut(&id) {
            spell.power_type = 0;
            spell.mana_cost = mana;
        }
    }
}

/// Rychlost strel vezi (yd/s): Axe Flurry, sekery Watch Tower a glevy (puvodne 40), Boulder,
/// katapult, delo a Treant (puvodne 30).
const HD_SPELL_SPEEDS: [(u32, f32); 2] = [(24020, 20.0), (9483, 15.0)];

/// Vizualni aury vezi: (kouzlo, jmeno, ikona, text). Cisla jako `HdVisual` v serveru (`hd_defs.h`).
const HD_VISUAL_AURAS: [(u32, &str, &str, &str); 11] = [
    (12881, "Jed", "Spell_Nature_CorrosiveBreath", "Jed: poskozeni prirodou v case."),
    (28330, "Horeni", "Spell_Fire_Immolation", "Hori: poskozeni ohnem v case."),
    (23072, "Temna rana", "Spell_Shadow_ShadowWordPain", "Stinove poskozeni v case nebo vyssi prijate poskozeni."),
    (7742, "Mraz", "Spell_Frost_FrostBolt02", "Zpomaleny mrazem."),
    (6388, "Zpomaleni", "Spell_Nature_StrengthOfEarthTotem02", "Zpomaleny."),
    (28356, "Nemoc", "Spell_Shadow_CallofBone", "Dostava vic poskozeni."),
    (24152, "Korinky", "Spell_Nature_StrangleVines", "Nemuze se hnout."),
    (10017, "Zamrznuti", "Spell_Frost_FrostNova", "Zamrzly, nemuze se hnout."),
    (18970, "Omraceni", "Spell_Frost_Stun", "Omraceny."),
    (22751, "Bloodlust", "Spell_Nature_BloodLust", "Rychlejsi utok."),
    (22108, "Arkanove pretizeni", "Spell_Nature_WispSplode", "Kazdy stack: +10 % poskozeni a dosahu Arcane Explosion."),
];

/// Kouzla Warriora pro Melee a jejich cena v mane: Heroic Strike 1 a 2, Battle Shout, Charge,
/// Rend 1 a 2, Thunder Clap, Hamstring.
const HD_MANA_SPELLS: [(u32, u32); 8] =
    [(78, 30), (284, 30), (6673, 20), (100, 0), (772, 20), (6546, 20), (6343, 40), (1715, 20)];

/// Kouzlo svitku zkusenosti, musi sedet s hero_defense_loot.sql.
const XP_SCROLL_SPELL: u32 = 5202;
