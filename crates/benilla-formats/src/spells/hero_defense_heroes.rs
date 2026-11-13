//! Hero Defense: kouzla hrdinu (nazvy, popisy, cena, cas, dosah, hodnoty efektu).
//!
//! VYGENEROVANO z `core/tools/heroes/spells.csv` (`node tools/heroes/gen_spells.js`), rucne neupravovat.
//! Server ma stejna cisla v `core/sql/hero_defense/hero_defense_hero_spells.sql`.

use std::collections::HashMap;

use super::SpellDisplay;

fn set_effect(s: &mut SpellDisplay, i: usize, base_points: i32, die_sides: i32) {
    s.effect_base_points[i] = base_points;
    s.effect_die_sides[i] = die_sides;
    s.effect_base_dice[i] = 1;
}

fn set_radius(s: &mut SpellDisplay, radius_index: u32) {
    for r in s.effect_radius_index.iter_mut() {
        if *r > 0 {
            *r = radius_index;
        }
    }
}

pub(super) fn apply(spells: &mut HashMap<u32, SpellDisplay>) {
    // Heroic Strike 1 (level 1)
    if let Some(s) = spells.get_mut(&78) {
        s.name = "Heroic Strike".to_string();
        s.rank = Some("Rank 1".to_string());
        s.description = Some("Pristi uder zbrani da navic 15 poskozeni.".to_string());
        s.aura_description = Some("Pristi uder zbrani da navic 15 poskozeni.".to_string());
        s.power_type = 0;
        s.stances = 0;
        s.stances_not = 0;
        s.base_level = 1;
        s.spell_level = 1;
        s.max_level = 0;
        s.effect_dice_per_level = [0; 3];
        s.mana_cost = 10;
        set_effect(s, 0, 14, 1);
    }
    // Heroic Strike 2 (level 5)
    if let Some(s) = spells.get_mut(&284) {
        s.name = "Heroic Strike".to_string();
        s.rank = Some("Rank 2".to_string());
        s.description = Some("Pristi uder zbrani da navic 35 poskozeni.".to_string());
        s.aura_description = Some("Pristi uder zbrani da navic 35 poskozeni.".to_string());
        s.power_type = 0;
        s.stances = 0;
        s.stances_not = 0;
        s.base_level = 5;
        s.spell_level = 5;
        s.max_level = 0;
        s.effect_dice_per_level = [0; 3];
        s.mana_cost = 15;
        set_effect(s, 0, 34, 1);
    }
    if let Some(s) = spells.get_mut(&1606) {
        s.name = "Heroic Strike".to_string();
        s.rank = Some("Rank 2".to_string());
        s.description = Some("Pristi uder zbrani da navic 35 poskozeni.".to_string());
    }
    // Heroic Strike 3 (level 9)
    if let Some(s) = spells.get_mut(&285) {
        s.name = "Heroic Strike".to_string();
        s.rank = Some("Rank 3".to_string());
        s.description = Some("Pristi uder zbrani da navic 70 poskozeni.".to_string());
        s.aura_description = Some("Pristi uder zbrani da navic 70 poskozeni.".to_string());
        s.power_type = 0;
        s.stances = 0;
        s.stances_not = 0;
        s.base_level = 9;
        s.spell_level = 9;
        s.max_level = 0;
        s.effect_dice_per_level = [0; 3];
        s.mana_cost = 25;
        set_effect(s, 0, 69, 1);
    }
    if let Some(s) = spells.get_mut(&1607) {
        s.name = "Heroic Strike".to_string();
        s.rank = Some("Rank 3".to_string());
        s.description = Some("Pristi uder zbrani da navic 70 poskozeni.".to_string());
    }
    // Shield Bash 1 (level 2)
    if let Some(s) = spells.get_mut(&72) {
        s.name = "Shield Bash".to_string();
        s.rank = Some("Rank 1".to_string());
        s.description = Some("Uder, ktery da 20 poskozeni a omraci nepritele na 2 s. Stit neni potreba.".to_string());
        s.aura_description = Some("Uder, ktery da 20 poskozeni a omraci nepritele na 2 s. Stit neni potreba.".to_string());
        s.power_type = 0;
        s.stances = 0;
        s.stances_not = 0;
        s.base_level = 2;
        s.spell_level = 2;
        s.max_level = 0;
        s.effect_dice_per_level = [0; 3];
        s.mana_cost = 15;
        s.recovery_ms = 10000;
        s.category_recovery_ms = 0;
        s.category = 0;
        s.duration_index = 39;
        set_effect(s, 1, 19, 1);
        s.effects[2] = 6;
        s.effect_apply_aura[2] = 12;
        s.effect_implicit_target_a[2] = 6;
        s.effect_mechanic[2] = 12;
        s.equipped_item_class = -1;
        s.equipped_item_subclass_mask = 0;
        s.equipped_item_inventory_type_mask = 0;
    }
    if let Some(s) = spells.get_mut(&1675) {
        s.name = "Shield Bash".to_string();
        s.rank = Some("Rank 1".to_string());
        s.description = Some("Uder, ktery da 20 poskozeni a omraci nepritele na 2 s. Stit neni potreba.".to_string());
    }
    // Shield Bash 2 (level 5)
    if let Some(s) = spells.get_mut(&1671) {
        s.name = "Shield Bash".to_string();
        s.rank = Some("Rank 2".to_string());
        s.description = Some("Uder, ktery da 45 poskozeni a omraci nepritele na 2 s. Stit neni potreba.".to_string());
        s.aura_description = Some("Uder, ktery da 45 poskozeni a omraci nepritele na 2 s. Stit neni potreba.".to_string());
        s.power_type = 0;
        s.stances = 0;
        s.stances_not = 0;
        s.base_level = 5;
        s.spell_level = 5;
        s.max_level = 0;
        s.effect_dice_per_level = [0; 3];
        s.mana_cost = 20;
        s.recovery_ms = 10000;
        s.category_recovery_ms = 0;
        s.category = 0;
        s.duration_index = 39;
        set_effect(s, 1, 44, 1);
        s.effects[2] = 6;
        s.effect_apply_aura[2] = 12;
        s.effect_implicit_target_a[2] = 6;
        s.effect_mechanic[2] = 12;
        s.equipped_item_class = -1;
        s.equipped_item_subclass_mask = 0;
        s.equipped_item_inventory_type_mask = 0;
    }
    if let Some(s) = spells.get_mut(&1676) {
        s.name = "Shield Bash".to_string();
        s.rank = Some("Rank 2".to_string());
        s.description = Some("Uder, ktery da 45 poskozeni a omraci nepritele na 2 s. Stit neni potreba.".to_string());
    }
    // Shield Bash 3 (level 9)
    if let Some(s) = spells.get_mut(&1672) {
        s.name = "Shield Bash".to_string();
        s.rank = Some("Rank 3".to_string());
        s.description = Some("Uder, ktery da 90 poskozeni a omraci nepritele na 2 s. Stit neni potreba.".to_string());
        s.aura_description = Some("Uder, ktery da 90 poskozeni a omraci nepritele na 2 s. Stit neni potreba.".to_string());
        s.power_type = 0;
        s.stances = 0;
        s.stances_not = 0;
        s.base_level = 9;
        s.spell_level = 9;
        s.max_level = 0;
        s.effect_dice_per_level = [0; 3];
        s.mana_cost = 30;
        s.recovery_ms = 10000;
        s.category_recovery_ms = 0;
        s.category = 0;
        s.duration_index = 39;
        set_effect(s, 1, 89, 1);
        s.effects[2] = 6;
        s.effect_apply_aura[2] = 12;
        s.effect_implicit_target_a[2] = 6;
        s.effect_mechanic[2] = 12;
        s.equipped_item_class = -1;
        s.equipped_item_subclass_mask = 0;
        s.equipped_item_inventory_type_mask = 0;
    }
    if let Some(s) = spells.get_mut(&1677) {
        s.name = "Shield Bash".to_string();
        s.rank = Some("Rank 3".to_string());
        s.description = Some("Uder, ktery da 90 poskozeni a omraci nepritele na 2 s. Stit neni potreba.".to_string());
    }
    // Intercept (level 4)
    if let Some(s) = spells.get_mut(&20252) {
        s.name = "Intercept".to_string();
        s.rank = None;
        s.description = Some("Skok k nepriteli (8 az 25 yd), omraci ho na 3 s.".to_string());
        s.aura_description = Some("Skok k nepriteli (8 az 25 yd), omraci ho na 3 s.".to_string());
        s.power_type = 0;
        s.stances = 0;
        s.stances_not = 0;
        s.base_level = 4;
        s.spell_level = 4;
        s.max_level = 0;
        s.effect_dice_per_level = [0; 3];
        s.mana_cost = 20;
        s.recovery_ms = 15000;
        s.category_recovery_ms = 0;
        s.category = 0;
    }
    if let Some(s) = spells.get_mut(&20611) {
        s.name = "Intercept".to_string();
        s.rank = None;
        s.description = Some("Skok k nepriteli (8 az 25 yd), omraci ho na 3 s.".to_string());
    }
    // Cleave (level 6)
    if let Some(s) = spells.get_mut(&845) {
        s.name = "Cleave".to_string();
        s.rank = None;
        s.description = Some("Pristi uder zasahne az 3 nepratele pred tebou a da navic 30 poskozeni.".to_string());
        s.aura_description = Some("Pristi uder zasahne az 3 nepratele pred tebou a da navic 30 poskozeni.".to_string());
        s.power_type = 0;
        s.stances = 0;
        s.stances_not = 0;
        s.base_level = 6;
        s.spell_level = 6;
        s.max_level = 0;
        s.effect_dice_per_level = [0; 3];
        s.mana_cost = 20;
        set_effect(s, 0, 29, 1);
    }
    if let Some(s) = spells.get_mut(&797) {
        s.name = "Cleave".to_string();
        s.rank = None;
        s.description = Some("Pristi uder zasahne az 3 nepratele pred tebou a da navic 30 poskozeni.".to_string());
    }
    // Execute (level 8)
    if let Some(s) = spells.get_mut(&5308) {
        s.name = "Execute".to_string();
        s.rank = None;
        s.description = Some("Zdrcujici uder za 200 poskozeni. Jen na nepritele pod 20 % zivota.".to_string());
        s.aura_description = Some("Zdrcujici uder za 200 poskozeni. Jen na nepritele pod 20 % zivota.".to_string());
        s.power_type = 0;
        s.stances = 0;
        s.stances_not = 0;
        s.base_level = 8;
        s.spell_level = 8;
        s.max_level = 0;
        s.effect_dice_per_level = [0; 3];
        s.mana_cost = 20;
        set_effect(s, 0, 199, 1);
    }
    if let Some(s) = spells.get_mut(&5283) {
        s.name = "Execute".to_string();
        s.rank = None;
        s.description = Some("Zdrcujici uder za 200 poskozeni. Jen na nepritele pod 20 % zivota.".to_string());
    }
    // Whirlwind (level 10)
    if let Some(s) = spells.get_mut(&1680) {
        s.name = "Whirlwind".to_string();
        s.rank = None;
        s.description = Some("Vir: zasahne vsechny nepratele do 8 yd zbrani a navic 40 poskozeni.".to_string());
        s.aura_description = Some("Vir: zasahne vsechny nepratele do 8 yd zbrani a navic 40 poskozeni.".to_string());
        s.power_type = 0;
        s.stances = 0;
        s.stances_not = 0;
        s.base_level = 10;
        s.spell_level = 10;
        s.max_level = 0;
        s.effect_dice_per_level = [0; 3];
        s.mana_cost = 35;
        s.recovery_ms = 8000;
        s.category_recovery_ms = 0;
        s.category = 0;
        set_radius(s, 14);
        set_effect(s, 0, 39, 1);
    }
    if let Some(s) = spells.get_mut(&1685) {
        s.name = "Whirlwind".to_string();
        s.rank = None;
        s.description = Some("Vir: zasahne vsechny nepratele do 8 yd zbrani a navic 40 poskozeni.".to_string());
    }
    // Arcane Shot 1 (level 1)
    if let Some(s) = spells.get_mut(&3044) {
        s.name = "Arcane Shot".to_string();
        s.rank = Some("Rank 1".to_string());
        s.description = Some("Rychla strela za 25 poskozeni (arkanni).".to_string());
        s.aura_description = Some("Rychla strela za 25 poskozeni (arkanni).".to_string());
        s.power_type = 0;
        s.stances = 0;
        s.stances_not = 0;
        s.base_level = 1;
        s.spell_level = 1;
        s.max_level = 0;
        s.effect_dice_per_level = [0; 3];
        s.mana_cost = 10;
        s.recovery_ms = 3000;
        s.category_recovery_ms = 0;
        s.category = 0;
        s.range_index = 35;
        set_effect(s, 0, 24, 1);
    }
    // Arcane Shot 2 (level 5)
    if let Some(s) = spells.get_mut(&14281) {
        s.name = "Arcane Shot".to_string();
        s.rank = Some("Rank 2".to_string());
        s.description = Some("Rychla strela za 55 poskozeni (arkanni).".to_string());
        s.aura_description = Some("Rychla strela za 55 poskozeni (arkanni).".to_string());
        s.power_type = 0;
        s.stances = 0;
        s.stances_not = 0;
        s.base_level = 5;
        s.spell_level = 5;
        s.max_level = 0;
        s.effect_dice_per_level = [0; 3];
        s.mana_cost = 15;
        s.recovery_ms = 3000;
        s.category_recovery_ms = 0;
        s.category = 0;
        s.range_index = 35;
        set_effect(s, 0, 54, 1);
    }
    if let Some(s) = spells.get_mut(&14352) {
        s.name = "Arcane Shot".to_string();
        s.rank = Some("Rank 2".to_string());
        s.description = Some("Rychla strela za 55 poskozeni (arkanni).".to_string());
    }
    // Arcane Shot 3 (level 9)
    if let Some(s) = spells.get_mut(&14282) {
        s.name = "Arcane Shot".to_string();
        s.rank = Some("Rank 3".to_string());
        s.description = Some("Rychla strela za 110 poskozeni (arkanni).".to_string());
        s.aura_description = Some("Rychla strela za 110 poskozeni (arkanni).".to_string());
        s.power_type = 0;
        s.stances = 0;
        s.stances_not = 0;
        s.base_level = 9;
        s.spell_level = 9;
        s.max_level = 0;
        s.effect_dice_per_level = [0; 3];
        s.mana_cost = 25;
        s.recovery_ms = 3000;
        s.category_recovery_ms = 0;
        s.category = 0;
        s.range_index = 35;
        set_effect(s, 0, 109, 1);
    }
    if let Some(s) = spells.get_mut(&14353) {
        s.name = "Arcane Shot".to_string();
        s.rank = Some("Rank 3".to_string());
        s.description = Some("Rychla strela za 110 poskozeni (arkanni).".to_string());
    }
    // Concussive Shot (level 2)
    if let Some(s) = spells.get_mut(&5116) {
        s.name = "Concussive Shot".to_string();
        s.rank = None;
        s.description = Some("Zpomali nepritele o 50 % na 4 s.".to_string());
        s.aura_description = Some("Zpomali nepritele o 50 % na 4 s.".to_string());
        s.power_type = 0;
        s.stances = 0;
        s.stances_not = 0;
        s.base_level = 2;
        s.spell_level = 2;
        s.max_level = 0;
        s.effect_dice_per_level = [0; 3];
        s.mana_cost = 15;
        s.recovery_ms = 8000;
        s.category_recovery_ms = 0;
        s.category = 0;
        s.range_index = 35;
        s.duration_index = 35;
        s.effect_apply_aura[0] = 4;
    }
    if let Some(s) = spells.get_mut(&5117) {
        s.name = "Concussive Shot".to_string();
        s.rank = None;
        s.description = Some("Zpomali nepritele o 50 % na 4 s.".to_string());
    }
    // Hunter's Mark (level 4)
    if let Some(s) = spells.get_mut(&1130) {
        s.name = "Hunter's Mark".to_string();
        s.rank = None;
        s.description = Some("Oznaceny nepritel dostava o 20 % vic poskozeni od vseho (hrdinove i veze) po 20 s.".to_string());
        s.aura_description = Some("Oznaceny nepritel dostava o 20 % vic poskozeni od vseho (hrdinove i veze) po 20 s.".to_string());
        s.power_type = 0;
        s.stances = 0;
        s.stances_not = 0;
        s.base_level = 4;
        s.spell_level = 4;
        s.max_level = 0;
        s.effect_dice_per_level = [0; 3];
        s.mana_cost = 15;
        s.range_index = 35;
        s.duration_index = 18;
    }
    if let Some(s) = spells.get_mut(&5298) {
        s.name = "Hunter's Mark".to_string();
        s.rank = None;
        s.description = Some("Oznaceny nepritel dostava o 20 % vic poskozeni od vseho (hrdinove i veze) po 20 s.".to_string());
    }
    // Frost Trap (level 6)
    if let Some(s) = spells.get_mut(&13809) {
        s.name = "Frost Trap".to_string();
        s.rank = None;
        s.description = Some("Polozi pod sebe mrazivou past. Kdo jde pres ni, zpomali se o 60 %. Past vydrzi 60 s.".to_string());
        s.aura_description = Some("Polozi pod sebe mrazivou past. Kdo jde pres ni, zpomali se o 60 %. Past vydrzi 60 s.".to_string());
        s.power_type = 0;
        s.stances = 0;
        s.stances_not = 0;
        s.base_level = 6;
        s.spell_level = 6;
        s.max_level = 0;
        s.effect_dice_per_level = [0; 3];
        s.mana_cost = 20;
        s.recovery_ms = 15000;
        s.category_recovery_ms = 0;
        s.category = 0;
    }
    if let Some(s) = spells.get_mut(&13811) {
        s.name = "Frost Trap".to_string();
        s.rank = None;
        s.description = Some("Polozi pod sebe mrazivou past. Kdo jde pres ni, zpomali se o 60 %. Past vydrzi 60 s.".to_string());
    }
    // Multi-Shot (level 8)
    if let Some(s) = spells.get_mut(&2643) {
        s.name = "Multi-Shot".to_string();
        s.rank = None;
        s.description = Some("Strela zasahne 3 nepratele a da navic 40 poskozeni.".to_string());
        s.aura_description = Some("Strela zasahne 3 nepratele a da navic 40 poskozeni.".to_string());
        s.power_type = 0;
        s.stances = 0;
        s.stances_not = 0;
        s.base_level = 8;
        s.spell_level = 8;
        s.max_level = 0;
        s.effect_dice_per_level = [0; 3];
        s.mana_cost = 25;
        s.recovery_ms = 6000;
        s.category_recovery_ms = 0;
        s.category = 0;
        s.range_index = 35;
        set_effect(s, 0, 39, 1);
    }
    if let Some(s) = spells.get_mut(&5338) {
        s.name = "Multi-Shot".to_string();
        s.rank = None;
        s.description = Some("Strela zasahne 3 nepratele a da navic 40 poskozeni.".to_string());
    }
    // Aimed Shot (level 10)
    if let Some(s) = spells.get_mut(&19434) {
        s.name = "Aimed Shot".to_string();
        s.rank = None;
        s.description = Some("Mirena strela za 250 poskozeni. Sesilani 3 s.".to_string());
        s.aura_description = Some("Mirena strela za 250 poskozeni. Sesilani 3 s.".to_string());
        s.power_type = 0;
        s.stances = 0;
        s.stances_not = 0;
        s.base_level = 10;
        s.spell_level = 10;
        s.max_level = 0;
        s.effect_dice_per_level = [0; 3];
        s.mana_cost = 35;
        s.casting_time_index = 14;
        s.recovery_ms = 6000;
        s.category_recovery_ms = 0;
        s.category = 0;
        s.range_index = 35;
        set_effect(s, 0, 249, 1);
    }
    if let Some(s) = spells.get_mut(&20931) {
        s.name = "Aimed Shot".to_string();
        s.rank = None;
        s.description = Some("Mirena strela za 250 poskozeni. Sesilani 3 s.".to_string());
        s.effect_trigger_spell[0] = 19434;
    }
    // Fireball 1 (level 1)
    if let Some(s) = spells.get_mut(&133) {
        s.name = "Fireball".to_string();
        s.rank = Some("Rank 1".to_string());
        s.description = Some("Ohniva koule za 20 az 28 poskozeni.".to_string());
        s.aura_description = Some("Ohniva koule za 20 az 28 poskozeni.".to_string());
        s.power_type = 0;
        s.stances = 0;
        s.stances_not = 0;
        s.base_level = 1;
        s.spell_level = 1;
        s.max_level = 0;
        s.effect_dice_per_level = [0; 3];
        s.mana_cost = 12;
        s.casting_time_index = 16;
        s.range_index = 35;
        set_effect(s, 0, 19, 9);
    }
    // Fireball 2 (level 5)
    if let Some(s) = spells.get_mut(&143) {
        s.name = "Fireball".to_string();
        s.rank = Some("Rank 2".to_string());
        s.description = Some("Ohniva koule za 45 az 55 poskozeni.".to_string());
        s.aura_description = Some("Ohniva koule za 45 az 55 poskozeni.".to_string());
        s.power_type = 0;
        s.stances = 0;
        s.stances_not = 0;
        s.base_level = 5;
        s.spell_level = 5;
        s.max_level = 0;
        s.effect_dice_per_level = [0; 3];
        s.mana_cost = 20;
        s.casting_time_index = 16;
        s.range_index = 35;
        set_effect(s, 0, 44, 11);
    }
    if let Some(s) = spells.get_mut(&483) {
        s.name = "Fireball".to_string();
        s.rank = Some("Rank 2".to_string());
        s.description = Some("Ohniva koule za 45 az 55 poskozeni.".to_string());
    }
    // Fireball 3 (level 9)
    if let Some(s) = spells.get_mut(&145) {
        s.name = "Fireball".to_string();
        s.rank = Some("Rank 3".to_string());
        s.description = Some("Ohniva koule za 85 az 105 poskozeni.".to_string());
        s.aura_description = Some("Ohniva koule za 85 az 105 poskozeni.".to_string());
        s.power_type = 0;
        s.stances = 0;
        s.stances_not = 0;
        s.base_level = 9;
        s.spell_level = 9;
        s.max_level = 0;
        s.effect_dice_per_level = [0; 3];
        s.mana_cost = 32;
        s.casting_time_index = 16;
        s.range_index = 35;
        set_effect(s, 0, 84, 21);
    }
    if let Some(s) = spells.get_mut(&502) {
        s.name = "Fireball".to_string();
        s.rank = Some("Rank 3".to_string());
        s.description = Some("Ohniva koule za 85 az 105 poskozeni.".to_string());
    }
    // Fire Blast 1 (level 2)
    if let Some(s) = spells.get_mut(&2136) {
        s.name = "Fire Blast".to_string();
        s.rank = Some("Rank 1".to_string());
        s.description = Some("Okamzity vybuch ohne za 18 az 24 poskozeni.".to_string());
        s.aura_description = Some("Okamzity vybuch ohne za 18 az 24 poskozeni.".to_string());
        s.power_type = 0;
        s.stances = 0;
        s.stances_not = 0;
        s.base_level = 2;
        s.spell_level = 2;
        s.max_level = 0;
        s.effect_dice_per_level = [0; 3];
        s.mana_cost = 15;
        s.recovery_ms = 6000;
        s.category_recovery_ms = 0;
        s.category = 0;
        s.range_index = 3;
        set_effect(s, 0, 17, 7);
    }
    if let Some(s) = spells.get_mut(&2141) {
        s.name = "Fire Blast".to_string();
        s.rank = Some("Rank 1".to_string());
        s.description = Some("Okamzity vybuch ohne za 18 az 24 poskozeni.".to_string());
    }
    // Fire Blast 2 (level 5)
    if let Some(s) = spells.get_mut(&2137) {
        s.name = "Fire Blast".to_string();
        s.rank = Some("Rank 2".to_string());
        s.description = Some("Okamzity vybuch ohne za 40 az 50 poskozeni.".to_string());
        s.aura_description = Some("Okamzity vybuch ohne za 40 az 50 poskozeni.".to_string());
        s.power_type = 0;
        s.stances = 0;
        s.stances_not = 0;
        s.base_level = 5;
        s.spell_level = 5;
        s.max_level = 0;
        s.effect_dice_per_level = [0; 3];
        s.mana_cost = 22;
        s.recovery_ms = 6000;
        s.category_recovery_ms = 0;
        s.category = 0;
        s.range_index = 3;
        set_effect(s, 0, 39, 11);
    }
    if let Some(s) = spells.get_mut(&2142) {
        s.name = "Fire Blast".to_string();
        s.rank = Some("Rank 2".to_string());
        s.description = Some("Okamzity vybuch ohne za 40 az 50 poskozeni.".to_string());
    }
    // Fire Blast 3 (level 9)
    if let Some(s) = spells.get_mut(&2138) {
        s.name = "Fire Blast".to_string();
        s.rank = Some("Rank 3".to_string());
        s.description = Some("Okamzity vybuch ohne za 80 az 95 poskozeni.".to_string());
        s.aura_description = Some("Okamzity vybuch ohne za 80 az 95 poskozeni.".to_string());
        s.power_type = 0;
        s.stances = 0;
        s.stances_not = 0;
        s.base_level = 9;
        s.spell_level = 9;
        s.max_level = 0;
        s.effect_dice_per_level = [0; 3];
        s.mana_cost = 32;
        s.recovery_ms = 6000;
        s.category_recovery_ms = 0;
        s.category = 0;
        s.range_index = 3;
        set_effect(s, 0, 79, 16);
    }
    if let Some(s) = spells.get_mut(&2143) {
        s.name = "Fire Blast".to_string();
        s.rank = Some("Rank 3".to_string());
        s.description = Some("Okamzity vybuch ohne za 80 az 95 poskozeni.".to_string());
    }
    // Flamestrike (level 4)
    if let Some(s) = spells.get_mut(&2120) {
        s.name = "Flamestrike".to_string();
        s.rank = None;
        s.description = Some("Ohen na zem: 35 az 45 poskozeni vsem do 8 yd a pak 5 za kazde 2 s po 8 s.".to_string());
        s.aura_description = Some("Ohen na zem: 35 az 45 poskozeni vsem do 8 yd a pak 5 za kazde 2 s po 8 s.".to_string());
        s.power_type = 0;
        s.stances = 0;
        s.stances_not = 0;
        s.base_level = 4;
        s.spell_level = 4;
        s.max_level = 0;
        s.effect_dice_per_level = [0; 3];
        s.mana_cost = 35;
        s.casting_time_index = 5;
        s.recovery_ms = 8000;
        s.category_recovery_ms = 0;
        s.category = 0;
        s.range_index = 4;
        s.duration_index = 31;
        set_radius(s, 14);
        set_effect(s, 0, 34, 11);
        set_effect(s, 1, 4, 1);
    }
    if let Some(s) = spells.get_mut(&846) {
        s.name = "Flamestrike".to_string();
        s.rank = None;
        s.description = Some("Ohen na zem: 35 az 45 poskozeni vsem do 8 yd a pak 5 za kazde 2 s po 8 s.".to_string());
    }
    // Frost Nova (level 6)
    if let Some(s) = spells.get_mut(&122) {
        s.name = "Frost Nova".to_string();
        s.rank = None;
        s.description = Some("Zmrazi nepratele do 10 yd na miste na 4 s a da jim 15 az 20 poskozeni.".to_string());
        s.aura_description = Some("Zmrazi nepratele do 10 yd na miste na 4 s a da jim 15 az 20 poskozeni.".to_string());
        s.power_type = 0;
        s.stances = 0;
        s.stances_not = 0;
        s.base_level = 6;
        s.spell_level = 6;
        s.max_level = 0;
        s.effect_dice_per_level = [0; 3];
        s.mana_cost = 30;
        s.recovery_ms = 15000;
        s.category_recovery_ms = 0;
        s.category = 0;
        s.duration_index = 35;
        set_radius(s, 13);
        set_effect(s, 0, 14, 6);
    }
    if let Some(s) = spells.get_mut(&497) {
        s.name = "Frost Nova".to_string();
        s.rank = None;
        s.description = Some("Zmrazi nepratele do 10 yd na miste na 4 s a da jim 15 az 20 poskozeni.".to_string());
    }
    // Blast Wave (level 8)
    if let Some(s) = spells.get_mut(&11113) {
        s.name = "Blast Wave".to_string();
        s.rank = None;
        s.description = Some("Tlakova vlna: 60 az 75 poskozeni vsem do 10 yd a odhodi je o 10 yd zpet po ceste.".to_string());
        s.aura_description = Some("Tlakova vlna: 60 az 75 poskozeni vsem do 10 yd a odhodi je o 10 yd zpet po ceste.".to_string());
        s.power_type = 0;
        s.stances = 0;
        s.stances_not = 0;
        s.base_level = 8;
        s.spell_level = 8;
        s.max_level = 0;
        s.effect_dice_per_level = [0; 3];
        s.mana_cost = 40;
        s.recovery_ms = 15000;
        s.category_recovery_ms = 0;
        s.category = 0;
        set_radius(s, 13);
        set_effect(s, 0, 59, 16);
        s.effects[1] = 0;
        s.effect_apply_aura[1] = 0;
    }
    if let Some(s) = spells.get_mut(&1831) {
        s.name = "Blast Wave".to_string();
        s.rank = None;
        s.description = Some("Tlakova vlna: 60 az 75 poskozeni vsem do 10 yd a odhodi je o 10 yd zpet po ceste.".to_string());
    }
    // Blizzard (level 10)
    if let Some(s) = spells.get_mut(&10) {
        s.name = "Blizzard".to_string();
        s.rank = None;
        s.description = Some("Ledova boure na zem: 30 poskozeni kazdou sekundu vsem do 8 yd po 8 s (kanal).".to_string());
        s.aura_description = Some("Ledova boure na zem: 30 poskozeni kazdou sekundu vsem do 8 yd po 8 s (kanal).".to_string());
        s.power_type = 0;
        s.stances = 0;
        s.stances_not = 0;
        s.base_level = 10;
        s.spell_level = 10;
        s.max_level = 0;
        s.effect_dice_per_level = [0; 3];
        s.mana_cost = 60;
        s.recovery_ms = 6000;
        s.category_recovery_ms = 0;
        s.category = 0;
        s.range_index = 4;
        s.duration_index = 31;
        set_radius(s, 14);
        set_effect(s, 0, 29, 1);
    }
    if let Some(s) = spells.get_mut(&1196) {
        s.name = "Blizzard".to_string();
        s.rank = None;
        s.description = Some("Ledova boure na zem: 30 poskozeni kazdou sekundu vsem do 8 yd po 8 s (kanal).".to_string());
    }
    // Corruption 1 (level 1)
    if let Some(s) = spells.get_mut(&172) {
        s.name = "Corruption".to_string();
        s.rank = Some("Rank 1".to_string());
        s.description = Some("Okamzite: 8 poskozeni kazde 3 s po 18 s (48).".to_string());
        s.aura_description = Some("Okamzite: 8 poskozeni kazde 3 s po 18 s (48).".to_string());
        s.power_type = 0;
        s.stances = 0;
        s.stances_not = 0;
        s.base_level = 1;
        s.spell_level = 1;
        s.max_level = 0;
        s.effect_dice_per_level = [0; 3];
        s.mana_cost = 12;
        s.casting_time_index = 1;
        s.range_index = 4;
        s.duration_index = 85;
        set_effect(s, 0, 7, 1);
    }
    // Corruption 2 (level 5)
    if let Some(s) = spells.get_mut(&6222) {
        s.name = "Corruption".to_string();
        s.rank = Some("Rank 2".to_string());
        s.description = Some("Okamzite: 18 poskozeni kazde 3 s po 18 s (108).".to_string());
        s.aura_description = Some("Okamzite: 18 poskozeni kazde 3 s po 18 s (108).".to_string());
        s.power_type = 0;
        s.stances = 0;
        s.stances_not = 0;
        s.base_level = 5;
        s.spell_level = 5;
        s.max_level = 0;
        s.effect_dice_per_level = [0; 3];
        s.mana_cost = 20;
        s.casting_time_index = 1;
        s.range_index = 4;
        s.duration_index = 85;
        set_effect(s, 0, 17, 1);
    }
    if let Some(s) = spells.get_mut(&6224) {
        s.name = "Corruption".to_string();
        s.rank = Some("Rank 2".to_string());
        s.description = Some("Okamzite: 18 poskozeni kazde 3 s po 18 s (108).".to_string());
    }
    // Corruption 3 (level 9)
    if let Some(s) = spells.get_mut(&6223) {
        s.name = "Corruption".to_string();
        s.rank = Some("Rank 3".to_string());
        s.description = Some("Okamzite: 35 poskozeni kazde 3 s po 18 s (210).".to_string());
        s.aura_description = Some("Okamzite: 35 poskozeni kazde 3 s po 18 s (210).".to_string());
        s.power_type = 0;
        s.stances = 0;
        s.stances_not = 0;
        s.base_level = 9;
        s.spell_level = 9;
        s.max_level = 0;
        s.effect_dice_per_level = [0; 3];
        s.mana_cost = 30;
        s.casting_time_index = 1;
        s.range_index = 4;
        s.duration_index = 85;
        set_effect(s, 0, 34, 1);
    }
    if let Some(s) = spells.get_mut(&6225) {
        s.name = "Corruption".to_string();
        s.rank = Some("Rank 3".to_string());
        s.description = Some("Okamzite: 35 poskozeni kazde 3 s po 18 s (210).".to_string());
    }
    // Drain Life 1 (level 2)
    if let Some(s) = spells.get_mut(&689) {
        s.name = "Drain Life".to_string();
        s.rank = Some("Rank 1".to_string());
        s.description = Some("Kanal: vysava 6 zivota kazdou sekundu po 5 s a leci te o ne.".to_string());
        s.aura_description = Some("Kanal: vysava 6 zivota kazdou sekundu po 5 s a leci te o ne.".to_string());
        s.power_type = 0;
        s.stances = 0;
        s.stances_not = 0;
        s.base_level = 2;
        s.spell_level = 2;
        s.max_level = 0;
        s.effect_dice_per_level = [0; 3];
        s.mana_cost = 15;
        s.range_index = 4;
        s.duration_index = 28;
        set_effect(s, 0, 5, 1);
    }
    if let Some(s) = spells.get_mut(&714) {
        s.name = "Drain Life".to_string();
        s.rank = Some("Rank 1".to_string());
        s.description = Some("Kanal: vysava 6 zivota kazdou sekundu po 5 s a leci te o ne.".to_string());
    }
    // Drain Life 2 (level 5)
    if let Some(s) = spells.get_mut(&699) {
        s.name = "Drain Life".to_string();
        s.rank = Some("Rank 2".to_string());
        s.description = Some("Kanal: vysava 12 zivota kazdou sekundu po 5 s a leci te o ne.".to_string());
        s.aura_description = Some("Kanal: vysava 12 zivota kazdou sekundu po 5 s a leci te o ne.".to_string());
        s.power_type = 0;
        s.stances = 0;
        s.stances_not = 0;
        s.base_level = 5;
        s.spell_level = 5;
        s.max_level = 0;
        s.effect_dice_per_level = [0; 3];
        s.mana_cost = 22;
        s.range_index = 4;
        s.duration_index = 28;
        set_effect(s, 0, 11, 1);
    }
    if let Some(s) = spells.get_mut(&725) {
        s.name = "Drain Life".to_string();
        s.rank = Some("Rank 2".to_string());
        s.description = Some("Kanal: vysava 12 zivota kazdou sekundu po 5 s a leci te o ne.".to_string());
    }
    // Drain Life 3 (level 9)
    if let Some(s) = spells.get_mut(&709) {
        s.name = "Drain Life".to_string();
        s.rank = Some("Rank 3".to_string());
        s.description = Some("Kanal: vysava 22 zivota kazdou sekundu po 5 s a leci te o ne.".to_string());
        s.aura_description = Some("Kanal: vysava 22 zivota kazdou sekundu po 5 s a leci te o ne.".to_string());
        s.power_type = 0;
        s.stances = 0;
        s.stances_not = 0;
        s.base_level = 9;
        s.spell_level = 9;
        s.max_level = 0;
        s.effect_dice_per_level = [0; 3];
        s.mana_cost = 32;
        s.range_index = 4;
        s.duration_index = 28;
        set_effect(s, 0, 21, 1);
    }
    if let Some(s) = spells.get_mut(&736) {
        s.name = "Drain Life".to_string();
        s.rank = Some("Rank 3".to_string());
        s.description = Some("Kanal: vysava 22 zivota kazdou sekundu po 5 s a leci te o ne.".to_string());
    }
    // Curse of Agony (level 4)
    if let Some(s) = spells.get_mut(&980) {
        s.name = "Curse of Agony".to_string();
        s.rank = None;
        s.description = Some("Kletba: 10 poskozeni kazde 2 s po 24 s (120).".to_string());
        s.aura_description = Some("Kletba: 10 poskozeni kazde 2 s po 24 s (120).".to_string());
        s.power_type = 0;
        s.stances = 0;
        s.stances_not = 0;
        s.base_level = 4;
        s.spell_level = 4;
        s.max_level = 0;
        s.effect_dice_per_level = [0; 3];
        s.mana_cost = 20;
        s.range_index = 4;
        s.duration_index = 106;
        set_effect(s, 0, 9, 1);
    }
    if let Some(s) = spells.get_mut(&981) {
        s.name = "Curse of Agony".to_string();
        s.rank = None;
        s.description = Some("Kletba: 10 poskozeni kazde 2 s po 24 s (120).".to_string());
    }
    // Curse of Weakness (level 6)
    if let Some(s) = spells.get_mut(&702) {
        s.name = "Curse of Weakness".to_string();
        s.rank = None;
        s.description = Some("Kletba: kdyz nepritel dojde k hradu, ubere mu o 50 % mene zivota. Trva 2 min.".to_string());
        s.aura_description = Some("Kletba: kdyz nepritel dojde k hradu, ubere mu o 50 % mene zivota. Trva 2 min.".to_string());
        s.power_type = 0;
        s.stances = 0;
        s.stances_not = 0;
        s.base_level = 6;
        s.spell_level = 6;
        s.max_level = 0;
        s.effect_dice_per_level = [0; 3];
        s.mana_cost = 15;
        s.range_index = 4;
        s.duration_index = 4;
    }
    if let Some(s) = spells.get_mut(&729) {
        s.name = "Curse of Weakness".to_string();
        s.rank = None;
        s.description = Some("Kletba: kdyz nepritel dojde k hradu, ubere mu o 50 % mene zivota. Trva 2 min.".to_string());
    }
    // Siphon Life (level 8)
    if let Some(s) = spells.get_mut(&18265) {
        s.name = "Siphon Life".to_string();
        s.rank = None;
        s.description = Some("Vysava 15 zivota kazde 3 s po 30 s a leci te o ne.".to_string());
        s.aura_description = Some("Vysava 15 zivota kazde 3 s po 30 s a leci te o ne.".to_string());
        s.power_type = 0;
        s.stances = 0;
        s.stances_not = 0;
        s.base_level = 8;
        s.spell_level = 8;
        s.max_level = 0;
        s.effect_dice_per_level = [0; 3];
        s.mana_cost = 30;
        s.range_index = 4;
        s.duration_index = 9;
        set_effect(s, 0, 14, 1);
    }
    if let Some(s) = spells.get_mut(&18927) {
        s.name = "Siphon Life".to_string();
        s.rank = None;
        s.description = Some("Vysava 15 zivota kazde 3 s po 30 s a leci te o ne.".to_string());
        s.effect_trigger_spell[0] = 18265;
    }
    // Epidemic (level 10)
    if let Some(s) = spells.get_mut(&6789) {
        s.name = "Epidemic".to_string();
        s.rank = None;
        s.description = Some("30 az 40 poskozeni a tve DoTy a kletby z cile preskoci na az 6 nepratel do 10 yd.".to_string());
        s.aura_description = Some("30 az 40 poskozeni a tve DoTy a kletby z cile preskoci na az 6 nepratel do 10 yd.".to_string());
        s.power_type = 0;
        s.stances = 0;
        s.stances_not = 0;
        s.base_level = 10;
        s.spell_level = 10;
        s.max_level = 0;
        s.effect_dice_per_level = [0; 3];
        s.mana_cost = 40;
        s.recovery_ms = 10000;
        s.category_recovery_ms = 0;
        s.category = 0;
        s.range_index = 4;
        set_effect(s, 0, 29, 11);
        s.effects[0] = 2;
        s.effect_apply_aura[0] = 0;
        s.effects[1] = 0;
        s.effect_apply_aura[1] = 0;
        s.effect_implicit_target_a[1] = 0;
        s.mechanic = 0;
        s.effect_mechanic[0] = 0;
        s.effect_mechanic[1] = 0;
    }
    if let Some(s) = spells.get_mut(&1572) {
        s.name = "Epidemic".to_string();
        s.rank = None;
        s.description = Some("30 az 40 poskozeni a tve DoTy a kletby z cile preskoci na az 6 nepratel do 10 yd.".to_string());
    }
    if let Some(s) = spells.get_mut(&13810) {
        s.effect_apply_aura[0] = 4;
    }
}
