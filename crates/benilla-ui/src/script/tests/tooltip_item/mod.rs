//! The engine item-tooltip renderer (decision 0274 P1, law per 0276): the verified line law
//! end-to-end, the red failed-requirement law against live player state, the sell-price money
//! protocol (engine fires `OnTooltipAddMoney` — merchant open + real instance + repair off),
//! and the in-flight-template fallback.

use super::common::script as bare_script;
use crate::script::*;

/// A stand-in `GlobalStrings.lua` for the item builder — **deliberately not the shipped wording**.
///
/// Every sentence the builder draws is a key resolved off the VM's own globals,
/// and what these tests establish is *which key* each line reaches and what fills it, never what
/// the sentence says. Naming each value after its own key is the point: `INVTYPE_SHIELD`,
/// `INVTYPE_WEAPONOFFHAND` and `SECONDARYHANDSLOT` all read "Off Hand" in enUS, `ITEM_REQ_SKILL`
/// shares "Requires %s" with the three `LOCKED_WITH_*` keys, and an assertion on the English
/// would pass on every one of them. Read an expected line here as "this key, filled with these".
///
/// The values keep each shipped template's *hole shape* — that is what proves the fill — and
/// nothing else about it.
fn seed_item_strings(s: &mut UiScript) {
    s.run(
        r#"
        CURRENTLY_EQUIPPED    = "[CURRENTLY_EQUIPPED]"
        GUILD_CHARTER_TITLE   = "[GUILD_CHARTER_TITLE %s]"
        GUILD_CHARTER_CREATOR = "[GUILD_CHARTER_CREATOR %s]"
        PETITION_TITLE        = "[PETITION_TITLE %s]"
        PETITION_CREATOR      = "[PETITION_CREATOR %s]"
        ITEM_SIGNABLE         = "[ITEM_SIGNABLE]"
        ITEM_CONJURED         = "[ITEM_CONJURED]"
        ITEM_SOULBOUND        = "[ITEM_SOULBOUND]"
        ITEM_BIND_QUEST       = "[ITEM_BIND_QUEST]"
        ITEM_BIND_ON_PICKUP   = "[ITEM_BIND_ON_PICKUP]"
        ITEM_BIND_ON_EQUIP    = "[ITEM_BIND_ON_EQUIP]"
        ITEM_BIND_ON_USE      = "[ITEM_BIND_ON_USE]"
        ITEM_UNIQUE           = "[ITEM_UNIQUE]"
        ITEM_UNIQUE_MULTIPLE  = "[ITEM_UNIQUE_MULTIPLE %d]"
        ITEM_STARTS_QUEST     = "[ITEM_STARTS_QUEST]"
        LOCKED                = "[LOCKED]"

        INVTYPE_HEAD = "[INVTYPE_HEAD]"; INVTYPE_NECK = "[INVTYPE_NECK]"
        INVTYPE_SHOULDER = "[INVTYPE_SHOULDER]"; INVTYPE_BODY = "[INVTYPE_BODY]"
        INVTYPE_CHEST = "[INVTYPE_CHEST]"; INVTYPE_WAIST = "[INVTYPE_WAIST]"
        INVTYPE_LEGS = "[INVTYPE_LEGS]"; INVTYPE_FEET = "[INVTYPE_FEET]"
        INVTYPE_WRIST = "[INVTYPE_WRIST]"; INVTYPE_HAND = "[INVTYPE_HAND]"
        INVTYPE_FINGER = "[INVTYPE_FINGER]"; INVTYPE_TRINKET = "[INVTYPE_TRINKET]"
        INVTYPE_WEAPON = "[INVTYPE_WEAPON]"; INVTYPE_SHIELD = "[INVTYPE_SHIELD]"
        INVTYPE_RANGED = "[INVTYPE_RANGED]"; INVTYPE_CLOAK = "[INVTYPE_CLOAK]"
        INVTYPE_2HWEAPON = "[INVTYPE_2HWEAPON]"; INVTYPE_TABARD = "[INVTYPE_TABARD]"
        INVTYPE_ROBE = "[INVTYPE_ROBE]"; INVTYPE_WEAPONMAINHAND = "[INVTYPE_WEAPONMAINHAND]"
        INVTYPE_WEAPONOFFHAND = "[INVTYPE_WEAPONOFFHAND]"; INVTYPE_HOLDABLE = "[INVTYPE_HOLDABLE]"
        INVTYPE_RELIC = "[INVTYPE_RELIC]"
        -- INVTYPE_AMMO / _THROWN / _RANGEDRIGHT / _QUIVER are DELIBERATELY absent: 1.12's
        -- GlobalStrings.lua does not ship them either, so those four types draw no slot cell.

        SPELL_SCHOOL1_CAP = "[SCHOOL1]"; SPELL_SCHOOL2_CAP = "[SCHOOL2]"
        SPELL_SCHOOL3_CAP = "[SCHOOL3]"; SPELL_SCHOOL4_CAP = "[SCHOOL4]"
        SPELL_SCHOOL5_CAP = "[SCHOOL5]"; SPELL_SCHOOL6_CAP = "[SCHOOL6]"

        DAMAGE_TEMPLATE                   = "[DMG %d - %d]"
        PLUS_DAMAGE_TEMPLATE              = "[+DMG %d - %d]"
        SINGLE_DAMAGE_TEMPLATE            = "[DMG1 %d]"
        PLUS_SINGLE_DAMAGE_TEMPLATE       = "[+DMG1 %d]"
        DAMAGE_TEMPLATE_WITH_SCHOOL       = "[DMGS %d - %d %s]"
        PLUS_DAMAGE_TEMPLATE_WITH_SCHOOL  = "[+DMGS %d - %d %s]"
        AMMO_DAMAGE_TEMPLATE              = "[AMMO %g]"
        PLUS_AMMO_DAMAGE_TEMPLATE         = "[+AMMO %g]"
        AMMO_SCHOOL_DAMAGE_TEMPLATE       = "[AMMOS %g %s]"
        PLUS_AMMO_SCHOOL_DAMAGE_TEMPLATE  = "[+AMMOS %g %s]"
        SPEED                 = "Speed"
        CONTAINER_SLOTS       = "[SLOTS %d %s]"
        DPS_TEMPLATE          = "[DPS %.1f]"
        ARMOR_TEMPLATE        = "[ARMOR %d]"
        SHIELD_BLOCK_TEMPLATE = "[BLOCK %d]"
        ITEM_MOD_MANA      = "[MOD_MANA %c%d]"
        ITEM_MOD_HEALTH    = "[MOD_HEALTH %c%d]"
        ITEM_MOD_AGILITY   = "[MOD_AGILITY %c%d]"
        ITEM_MOD_STRENGTH  = "[MOD_STRENGTH %c%d]"
        ITEM_MOD_INTELLECT = "[MOD_INTELLECT %c%d]"
        ITEM_MOD_SPIRIT    = "[MOD_SPIRIT %c%d]"
        ITEM_MOD_STAMINA   = "[MOD_STAMINA %c%d]"
        ITEM_RESIST_ALL    = "[RESIST_ALL %c%d]"
        ITEM_RESIST_SINGLE = "[RESIST_SINGLE %c%d %s]"

        ITEM_RANDOM_ENCHANT = "[ITEM_RANDOM_ENCHANT]"
        ITEM_ENCHANT_TIME_LEFT_DAYS     = "[ENCH %s %d d]"
        ITEM_ENCHANT_TIME_LEFT_DAYS_P1  = "[ENCH %s %d dd]"
        ITEM_ENCHANT_TIME_LEFT_HOURS    = "[ENCH %s %d h]"
        ITEM_ENCHANT_TIME_LEFT_HOURS_P1 = "[ENCH %s %d hh]"
        ITEM_ENCHANT_TIME_LEFT_MIN      = "[ENCH %s %d m]"
        ITEM_ENCHANT_TIME_LEFT_SEC      = "[ENCH %s %d s]"
        ITEM_SPELL_CHARGES    = "[CHARGES %d]"
        ITEM_SPELL_CHARGES_P1 = "[CHARGES %d x]"
        DURABILITY_TEMPLATE   = "[DURABILITY %d/%d]"
        -- ITEM_DURATION_* ships no _P1 twin, and neither does this stand-in.
        ITEM_DURATION_DAYS  = "[DURATION %d d]"
        ITEM_DURATION_HOURS = "[DURATION %d h]"
        ITEM_DURATION_MIN   = "[DURATION %d m]"
        ITEM_DURATION_SEC   = "[DURATION %d s]"

        ITEM_CLASSES_ALLOWED = "[CLASSES %s]"
        ITEM_RACES_ALLOWED   = "[RACES %s]"
        ITEM_MIN_LEVEL       = "[MIN_LEVEL %d]"
        ITEM_MIN_SKILL       = "[MIN_SKILL %s %d]"
        ITEM_REQ_SKILL       = "[REQ_SKILL %s]"
        ITEM_SPELL_KNOWN     = "[ITEM_SPELL_KNOWN]"
        ITEM_SPELL_TRIGGER_ONUSE   = "[ONUSE]"
        ITEM_SPELL_TRIGGER_ONEQUIP = "[ONEQUIP]"
        ITEM_SPELL_TRIGGER_ONPROC  = "[ONPROC]"

        ITEM_SET_NAME       = "[SET_NAME %s %d/%d]"
        ITEM_SET_BONUS      = "[SET_BONUS %s]"
        ITEM_SET_BONUS_GRAY = "[SET_BONUS_GRAY %d %s]"

        ITEM_WRITTEN_BY  = "[WRITTEN_BY %s]"
        ITEM_CREATED_BY  = "|cff00ff00[CREATED_BY %s]|r"
        ITEM_OPENABLE    = "[ITEM_OPENABLE]"
        ITEM_READABLE    = "[ITEM_READABLE]"
        ITEM_UNSELLABLE  = "[ITEM_UNSELLABLE]"
    "#,
    )
    .unwrap();
}

/// The shared fixture VM, with [`seed_item_strings`] already run — every test in this tree needs
/// it, because a builder that resolves nothing draws nothing.
fn script() -> UiScript {
    let mut s = bare_script();
    seed_item_strings(&mut s);
    s
}

/// A full weapon view exercising most line families.
fn axe() -> ItemTemplateView {
    ItemTemplateView {
        name: "Ravager".into(),
        quality: 3,
        class: 2,
        subclass: 1,
        // ItemSubClass.dbc's DisplayName for (2, 1) — the app-resolved word the type cell
        // prints; the two axe subclasses share it.
        sub_class_display: Some("Axe".into()),
        inventory_type: 17,
        bonding: 2,
        stats: vec![(7, 12), (4, 9)],
        damages: vec![(68.0, 103.0, 0), (2.0, 4.0, 5)],
        delay_ms: 3500,
        resistances: [0, 0, 0, 0, 10, 0],
        max_durability: 90,
        required_level: 37,
        allowable_class: (1 << 0) | (1 << 3), // Warrior, Rogue
        spell_triggers: vec![(2, 9633, "Ravager".into())],
        description: "A wicked axe of the Scarlet Crusade.".into(),
        sell_price: 15230,
        ..Default::default()
    }
}

/// The tooltip's left-column lines in order, each with its draw color (read from the extract —
/// the color a renderer would actually paint).
fn lines_of(s: &mut UiScript) -> Vec<(String, [f32; 4])> {
    s.resolve();
    let quads = s.extract();
    let count: i64 = s.eval("return TT:NumLines()").unwrap();
    (1..=count)
        .map(|i| {
            let t: String = s
                .eval(&format!(
                    "return getglobal('TTTextLeft{i}'):GetText() or ''"
                ))
                .unwrap();
            let color = quads
                .iter()
                .find_map(|q| match &q.content {
                    QuadContent::Text {
                        text: Some(txt),
                        color,
                        ..
                    } if *txt == t => *color,
                    _ => None,
                })
                .unwrap_or([0.0; 4]);
            (t, color)
        })
        .collect()
}

/// A right-cell color, matched by its unique text in the extract.
fn right_color(s: &mut UiScript, needle: &str) -> [f32; 4] {
    s.resolve();
    s.extract()
        .iter()
        .find_map(|q| match &q.content {
            QuadContent::Text {
                text: Some(t),
                color,
                ..
            } if t == needle => *color,
            _ => None,
        })
        .unwrap_or_else(|| panic!("no right cell {needle:?}"))
}

mod bindings;
mod law;
mod mechanics;
mod set;
