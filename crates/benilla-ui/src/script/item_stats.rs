//! The shared item-template store every item tooltip renders through, as the reference's one
//! renderer (`0x52b650`, behind 8 of the 9 `Set*Item` bindings) reads one template cache.
//!
//! The app pushes each template as it lands; a read of an unknown id records an ask that the app
//! turns into `CMSG_ITEM_QUERY`, the reference's uncached-item early-out. The app resolves every
//! DBC string, as the engine reads no DBCs.

use std::collections::HashMap;

use mlua::{Lua, MultiValue, Value};

use super::binding_abi;
use super::Model;

/// An item template's tooltip view: the `SMSG_ITEM_QUERY_SINGLE_RESPONSE` fields the tooltip reads,
/// plus app-resolved display strings.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ItemTemplateView {
    pub name: String,
    pub quality: u32,
    pub class: u32,
    pub subclass: u32,
    pub inventory_type: u32,
    /// `GetItemInfo`'s `itemType`, the `ItemClass.dbc` name (`0x48e070`); `None` answers `""`.
    pub item_type: Option<String>,
    /// `GetItemInfo`'s `itemSubType`, "One-Handed Swords": `ItemSubClass.dbc`'s VerboseName
    /// (`+0x4c`), or its DisplayName (`+0x28`) when that is empty (`0x48e311`).
    pub item_sub_type: Option<String>,
    /// The tooltip's type cell and the bag line's `CONTAINER_SLOTS` name: `ItemSubClass.dbc`'s
    /// DisplayName alone (`+0x28`, off the `0xc0db90` row cache), the singular "Sword".
    pub sub_class_display: Option<String>,
    /// The alternate subclass whose proficiency also permits use: `ItemSubClass.dbc`'s
    /// prerequisite, or its postrequisite when the prerequisite is -1. A weapon with only the
    /// alternate's bit reds the slot cell, not the type cell.
    pub proficiency_alt: Option<u32>,
    /// `ItemSubClass.dbc`'s prerequisite proficiency alone (row `+0x08`, `None` for -1):
    /// `GetTradeSkillItemStats` reads this field where the tooltip walks on to the postrequisite.
    pub proficiency_prereq: Option<u32>,
    /// The template's item level (`+0x38`).
    pub item_level: u32,
    /// `(spell name, charges)` for each of the five spell slots whose id names a `Spell.dbc` row,
    /// in slot order (`+0x11c`, `+0x144`): `GetTradeSkillItemStats`' `ITEM_SPELL_EFFECT` lines.
    pub spell_slots: Vec<(String, i32)>,
    /// `ItemSubClass.dbc` displayFlags bit 0: the type cell never prints (rings, trinkets, …).
    pub hide_subclass: bool,
    /// Template flags; `0x2` prints "Conjured Item".
    pub flags: u32,
    /// 1 binds on pickup, 2 on equip, 3 on use, 4 and 5 quest item.
    pub bonding: u32,
    /// `MaxCount`: 1 prints "Unique", N > 1 "Unique (N)"; not the stack size.
    pub max_count: u32,
    /// `Stackable`, `GetItemInfo`'s `itemStackCount` (`+0x60`, `0x48e28b`); `MaxCount` is `+0x5c`.
    pub stackable: u32,
    pub start_quest: u32,
    pub container_slots: u32,
    /// `(ItemModType, value)` in wire order, the `ITEM_MOD_*` lines.
    pub stats: Vec<(u32, i32)>,
    /// `(min, max, school)` in wire order.
    pub damages: Vec<(f32, f32, u32)>,
    pub delay_ms: u32,
    pub armor: u32,
    pub block: u32,
    /// Holy to Arcane.
    pub resistances: [i32; 6],
    pub max_durability: u32,
    /// "Requires Level N", printed only for N > 1 (`0x52d2cf`).
    pub required_level: u32,
    /// Class and race masks: `<= 0` prints no line, and a line missing the player's bit is red.
    pub allowable_class: i32,
    pub allowable_race: i32,
    /// `SkillLine.dbc` id, with its rank and app-resolved name; a `None` name prints no line.
    pub required_skill: u32,
    pub required_skill_rank: u32,
    pub required_skill_name: Option<String>,
    pub required_spell: u32,
    pub required_spell_name: Option<String>,
    /// No tooltip line in 1.12; only the usable gate (`0x5ea930`) reads it.
    pub required_honor_rank: u32,
    /// Usable gate only; nonzero always fails, as the `PVP_MEDALS` bits it tests are never set.
    pub required_city_rank: u32,
    /// "Requires <Faction> - <Standing>", app-resolved; the raw faction and rank drive its red.
    pub required_rep_line: Option<String>,
    pub required_rep_faction: u32,
    pub required_rep_rank: u32,
    /// Green trigger-spell lines `(trigger, spell id, text)` in wire order: 0 and 5 "Use:",
    /// 1 "Equip:", 2 "Chance on hit:", 6 a taught spell (no line, only the "Already known" red).
    pub spell_triggers: Vec<(u32, u32, String)>,
    pub lock_id: u32,
    /// "N Charge(s)" for the first spell slot past the builder's gate (`0x52db51`: 0 and -1
    /// print nothing, else the absolute value); 0 prints no line.
    pub charges: i32,
    pub description: String,
    pub page_text: u32,
    /// Copper: the merchant money row, or `ITEM_UNSELLABLE` when 0 in a sell context.
    pub sell_price: u32,
    pub item_set: u32,
    /// `GetItemInfo`'s `itemTexture`, the `Interface\Icons\…` path through `ItemDisplayInfo.dbc`
    /// (`0x48e2dd`); `None` for a display row with no icon.
    pub icon: Option<String>,
    /// `RandomProperty` (template `+0x1b8`): the item can roll a suffix, so a template tooltip
    /// prints `<Random enchantment>` (`0x52cc33`).
    pub random_property: u32,
}

/// The player state the tooltip's red lines and the usable gate compare against.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlayerReqState {
    pub level: u32,
    /// 1 warrior … 11 druid; the `allowable_class` bit is `1 << (id-1)`.
    pub class_id: u32,
    /// 1 human … 8 troll; the `allowable_race` bit is `1 << (id-1)`.
    pub race_id: u32,
    /// SkillLine id → value plus permanent bonus, what the gates compare.
    pub skills: HashMap<u32, u32>,
    /// Item class → `SMSG_SET_PROFICIENCY` subclass mask (`0xc4d4a0[class]`); a class with no
    /// entry never reds.
    pub proficiency: HashMap<u32, u32>,
    /// Faction id → reputation rank, 0 hated … 7 exalted.
    pub rep_ranks: HashMap<u32, u8>,
    /// Whether the spellbook holds a `SPELL_EFFECT_DUAL_WIELD` (40) spell, the client's
    /// `0xc4d770` (read at `0x5eab70`); without it an off-hand weapon reds its slot cell.
    pub can_dual_wield: bool,
    /// The highest lifetime honor rank (`PLAYER_FIELD_BYTES` byte 3).
    pub honor_rank: u8,
}

/// The icon an item-texture getter answers while the item's template is unknown or its display
/// has no icon: `0x5d8af0` looks the template up, finds none and passes display id 0 to `0x5d88b0`,
/// which falls back to `INV_Misc_QuestionMark` (`0x847fe4`, `0x5d8927`) under the icon directory.
/// `GetContainerItemInfo` (`0x4f9708`) and `GetTradePlayerItemInfo` (`0x4c0529`) push it, so their
/// texture is a string on every path.
pub(super) const MISSING_ITEM_ICON: &str = "Interface\\Icons\\INV_Misc_QuestionMark";

/// The quality an item-info getter answers: the template's quality only when the template is
/// cached and its InventoryType is nonzero, else -1 (`GetContainerItemInfo` `0x4f9794`..`0x4f97a6`,
/// `GetTradePlayerItemInfo` `0x4c059c`..`0x4c05af`, `GetTradeTargetItemInfo` `0x4c0241`). A potion,
/// a reagent or a trade good (InventoryType 0) answers -1 with its template in hand, and the
/// number is never nil. `inventory_type` is 0 while the template is unresolved.
pub(super) fn gated_quality(quality: Option<u32>, inventory_type: u32) -> i64 {
    match quality {
        Some(q) if inventory_type != 0 => i64::from(q),
        _ => -1,
    }
}

/// [`item_usable`] by item entry: entry 0 and a template not yet answered are usable, as the
/// merchant getter skips an uncached record (`0x4fb298`).
pub(super) fn item_usable_by_id(model: &super::Model, item_id: u32) -> bool {
    item_refusal_by_id(model, item_id).is_none()
}

/// [`item_refusal`] by item entry, with the same fail-open for entry 0 and an unanswered template.
pub(super) fn item_refusal_by_id(model: &super::Model, item_id: u32) -> Option<&'static str> {
    if item_id == 0 {
        return None;
    }
    model.item_templates.get(&item_id).and_then(|v| {
        item_refusal(v, &model.player_req, |id| {
            model.spellbook.slots.iter().any(|s| s.spell_id == id)
        })
    })
}

/// The client's item-usable predicate `0x5ea930`, whose answer the merchant getters push as
/// `isUsable` (`0x4fb2a3`, `0x4fb4f7`).
pub fn item_usable(
    v: &ItemTemplateView,
    req: &PlayerReqState,
    knows_spell: impl Fn(u32) -> bool,
) -> bool {
    item_refusal(v, req, knows_spell).is_none()
}

/// `0x5ea930`'s refusal, as the key of the error line its caller shows (`0x5d8e0e`, `0x5d926d`),
/// its legs in the reference's order: the proficiency leg is `ERR_PROFICIENCY_NEEDED` (`0x7`,
/// `0x5ea9b3`), every other failure `ERR_CANT_USE_ITEM` (`0x9c`, `0x5eaace`). Proficiency tests the
/// item's own subclass bit, with no alternate walk. A nonzero city rank always fails: it tests
/// `PLAYER_FIELD_PVP_MEDALS`, which is never written. Reputation compares ranks, equivalent to the
/// reference's standing threshold (`0x4d6370`, table `0x80928c`); an unknown faction is rank 0. An
/// unpushed state (level 0) declines to judge, answering usable.
pub fn item_refusal(
    v: &ItemTemplateView,
    req: &PlayerReqState,
    knows_spell: impl Fn(u32) -> bool,
) -> Option<&'static str> {
    const CANT_USE: Option<&str> = Some("ERR_CANT_USE_ITEM");
    if req.level == 0 {
        return None;
    }
    if v.required_level > req.level {
        return CANT_USE;
    }
    if req.class_id == 0 || v.allowable_class as u32 & (1 << (req.class_id - 1)) == 0 {
        return CANT_USE;
    }
    if req.race_id == 0 || v.allowable_race as u32 & (1 << (req.race_id - 1)) == 0 {
        return CANT_USE;
    }
    if let Some(&mask) = req.proficiency.get(&v.class) {
        if mask & (1 << v.subclass) == 0 {
            return Some("ERR_PROFICIENCY_NEEDED");
        }
    }
    if v.required_skill != 0 {
        match req.skills.get(&v.required_skill) {
            None => return CANT_USE,
            Some(&val) => {
                if val < v.required_skill_rank {
                    return CANT_USE;
                }
            }
        }
    }
    if v.required_spell != 0 && !knows_spell(v.required_spell) {
        return CANT_USE;
    }
    if v.required_honor_rank != 0 && u32::from(req.honor_rank) < v.required_honor_rank {
        return CANT_USE;
    }
    if v.required_city_rank != 0 {
        return CANT_USE;
    }
    if v.required_rep_faction != 0 {
        let rank = req
            .rep_ranks
            .get(&v.required_rep_faction)
            .copied()
            .unwrap_or(0);
        if u32::from(rank) < v.required_rep_rank {
            return CANT_USE;
        }
    }
    None
}

/// An item set's tooltip block (`0x854b1c`), app-resolved from `ItemSet.dbc`: member names from
/// the template cache and the bonuses' substituted text. The engine counts the equipped members.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ItemSetView {
    pub name: String,
    /// `(item id, name)` per member in DBC order; `None` while its template is uncached.
    pub members: Vec<(u32, Option<String>)>,
    /// `(required equipped count, bonus text)` in DBC slot order; the renderer sorts by count, as
    /// the builder's qsort does (`0x52e5c0`).
    pub bonuses: Vec<(u32, String)>,
    /// The set's "Requires <skill> (N)"; 0 prints no line.
    pub required_skill: u32,
    pub required_skill_rank: u32,
    pub required_skill_name: Option<String>,
}

impl super::UiScript {
    /// Stores or replaces an item's template view, clearing its ask and its read.
    pub fn set_item_template(&mut self, item_id: u32, view: ItemTemplateView) {
        let mut model = self.model_mut();
        model.item_stat_asks.remove(&item_id);
        model.item_stat_reads.remove(&item_id);
        model.item_templates.insert(item_id, view);
        model.quest_store.item_landed(item_id);
    }

    /// Asks for the templates of `ids` the store lacks, for the trade-skill and craft lists,
    /// whose link verbs never query: the reference has them cached before the list shows.
    pub fn ask_item_templates(&mut self, ids: impl IntoIterator<Item = u32>) {
        let mut model = self.model_mut();
        for id in ids {
            if id != 0 && !model.item_templates.contains_key(&id) {
                model.item_stat_asks.insert(id);
            }
        }
    }

    /// Drains the ids asked for that the store did not hold.
    pub fn take_item_stat_asks(&mut self) -> Vec<u32> {
        self.model_mut().item_stat_asks.drain().collect()
    }

    /// Drains the guids an item build missed in the name cache: a creator or a giver.
    pub fn take_item_name_asks(&mut self) -> Vec<u64> {
        self.model_mut().item_name_asks.drain().collect()
    }

    /// Drains the charter item guids an item build missed the petition record or owner of.
    pub fn take_petition_asks(&mut self) -> Vec<u64> {
        self.model_mut().petition_asks.drain().collect()
    }

    /// Drains the ids `GetItemInfo` read that the store did not hold, which the app serves from
    /// its cache and never asks the server for.
    pub fn take_item_stat_reads(&mut self) -> Vec<u32> {
        self.model_mut().item_stat_reads.drain().collect()
    }

    /// Stores or replaces a set's view, re-pushed as member names resolve.
    pub fn set_item_set(&mut self, set_id: u32, view: ItemSetView) {
        let mut model = self.model_mut();
        model.item_set_asks.remove(&set_id);
        model.item_sets.insert(set_id, view);
    }

    /// Drains the set ids the renderer asked for that the store did not hold.
    pub fn take_item_set_asks(&mut self) -> Vec<u32> {
        self.model_mut().item_set_asks.drain().collect()
    }

    /// Pushes the whole `ItemRandomProperties` table once, at load: a clicked tooltip cannot
    /// repaint on a late answer.
    pub fn set_random_properties(
        &mut self,
        rows: std::collections::HashMap<u32, super::RandomPropertyView>,
    ) {
        self.model_mut().random_properties = rows;
    }

    /// Pushes the player state the red lines compare against.
    pub fn set_player_req_state(&mut self, state: PlayerReqState) {
        self.model_mut().player_req = state;
    }
}

/// The 1.12 item-quality palette: the ARGB literals the static init `0x5291d0` writes to the BGRA
/// array `0xc0d3c8`, and the escape strings at `0x854124`. `GetItemQualityColor` (`0x48dfb0`)
/// answers each channel times 1/255.
const QUALITY_COLORS: [(u8, u8, u8, &str); 7] = [
    (0x9d, 0x9d, 0x9d, "|cff9d9d9d"), // 0 Poor
    (0xff, 0xff, 0xff, "|cffffffff"), // 1 Common
    (0x1e, 0xff, 0x00, "|cff1eff00"), // 2 Uncommon
    (0x00, 0x70, 0xdd, "|cff0070dd"), // 3 Rare
    (0xa3, 0x35, 0xee, "|cffa335ee"), // 4 Epic
    (0xff, 0x80, 0x00, "|cffff8000"), // 5 Legendary
    (0xe6, 0xcc, 0x80, "|cffe6cc80"), // 6 Artifact
];

/// The client's item-link builder `0x52adb0` as the trade-skill and craft link verbs call it:
/// `|c<rrggbb>|Hitem:<id>:0:0:0|h[<name>]|h|r`. A quality of 7 or more selects index 1, white,
/// by `0x52ad90`'s unsigned `cmp ecx,7; jb`.
pub(super) fn item_link(item_id: u32, name: &str, quality: u32) -> String {
    let hex = QUALITY_COLORS
        .get(quality as usize)
        .unwrap_or(&QUALITY_COLORS[1])
        .3;
    format!("{hex}|Hitem:{item_id}:0:0:0|h[{name}]|h|r")
}

/// `InventoryType` → `GetItemInfo`'s `itemEquipLoc`, the reference's 30-pointer table at
/// `0x83ddb0` (`0x48e29b`): slots 0 and 29 are the shared `""` (`0x882748`), not nil, and four
/// tokens (`AMMO`, `THROWN`, `RANGEDRIGHT`, `QUIVER`) have no `GlobalStrings.lua` entry.
fn equip_loc_token(inventory_type: u32) -> &'static str {
    const TOKENS: [&str; 29] = [
        "",
        "INVTYPE_HEAD",
        "INVTYPE_NECK",
        "INVTYPE_SHOULDER",
        "INVTYPE_BODY",
        "INVTYPE_CHEST",
        "INVTYPE_WAIST",
        "INVTYPE_LEGS",
        "INVTYPE_FEET",
        "INVTYPE_WRIST",
        "INVTYPE_HAND",
        "INVTYPE_FINGER",
        "INVTYPE_TRINKET",
        "INVTYPE_WEAPON",
        "INVTYPE_SHIELD",
        "INVTYPE_RANGED",
        "INVTYPE_CLOAK",
        "INVTYPE_2HWEAPON",
        "INVTYPE_BAG",
        "INVTYPE_TABARD",
        "INVTYPE_ROBE",
        "INVTYPE_WEAPONMAINHAND",
        "INVTYPE_WEAPONOFFHAND",
        "INVTYPE_HOLDABLE",
        "INVTYPE_AMMO",
        "INVTYPE_THROWN",
        "INVTYPE_RANGEDRIGHT",
        "INVTYPE_QUIVER",
        "INVTYPE_RELIC",
    ];
    // Deviation: bounds-checked where the reference indexes unguarded, so none reads past it.
    TOKENS.get(inventory_type as usize).copied().unwrap_or("")
}

/// `GetItemInfo`'s argument as the reference parses it (`0x48e0a3`-`0x48e16d`): a number, or a
/// string `lua_isnumber` (`0x6f34d0`) coerces, is the id alone; another string (`0x6f3510`) must
/// pass `strnicmp(s, "item:", 5)` (`0x64a4c0`) for its four fields, else it is id 0, so a name or
/// a hyperlink finds nothing; anything else raises the usage error (`0x842d24`, via `0x6f4940`).
fn parse_item_arg(v: &Value) -> mlua::Result<(i64, u32, u32, u32)> {
    let as_number = match v {
        Value::Integer(i) => Some(*i as f64),
        Value::Number(n) => Some(*n),
        Value::String(s) => s.to_str().ok().and_then(|s| s.trim().parse::<f64>().ok()),
        _ => None,
    };
    if let Some(n) = as_number {
        return Ok((n as i64, 0, 0, 0));
    }
    // An `item:` string; any other string is id 0.
    let Value::String(s) = v else {
        return Err(mlua::Error::RuntimeError(
            "Usage: GetItemInfo(itemID|\"itemlink\")".into(),
        ));
    };
    let s = s
        .to_str()
        .map_err(|_| mlua::Error::RuntimeError("Usage: GetItemInfo(itemID|\"itemlink\")".into()))?;
    let Some(rest) = s
        .get(..5)
        .filter(|p| p.eq_ignore_ascii_case("item:"))
        .map(|_| &s[5..])
    else {
        return Ok((0, 0, 0, 0));
    };
    let mut fields = rest
        .splitn(4, ':')
        .map(crate::script::binding_abi::s_str_to_int);
    let id = i64::from(fields.next().unwrap_or(0));
    let mut next = || u32::try_from(fields.next().unwrap_or(0)).unwrap_or(0);
    Ok((id, next(), next(), next()))
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    // GetItemQualityColor(quality) → r, g, b, escapeString (`0x48dfb0`). 7 and up, or a negative
    // quality, answer Common (`0x52ad70`/`0x52ad90`: an unsigned `cmp ecx,7; jb`), so
    // `ITEM_QUALITY_COLORS[-1]` is white, the colour of an uncached loot row (`0x4c2435`).
    lua.globals().set(
        "GetItemQualityColor",
        lua.create_function(|lua, quality: Value| {
            // `is-number` else `Usage: GetItemQualityColor(index)` (`0x842d00`, `0x48dfc5`).
            let quality = i64::from(binding_abi::number_arg(
                lua,
                quality,
                "Usage: GetItemQualityColor(index)",
            )?);
            // A 32-bit int (`__ftol`), compared unsigned.
            let i = match quality as i32 as u32 {
                q if q >= 7 => 1,
                q => q as usize,
            };
            let (r, g, b, hex) = QUALITY_COLORS[i];
            Ok(mlua::MultiValue::from_vec(vec![
                Value::Number(f64::from(r) / 255.0),
                Value::Number(f64::from(g) / 255.0),
                Value::Number(f64::from(b) / 255.0),
                Value::String(lua.create_string(hex)?),
            ]))
        })?,
    )?;

    // GetItemInfo(itemID | "item:id:enchant:randomProperty:suffix") → itemName, itemLink,
    // itemQuality, itemMinLevel, itemType, itemSubType, itemStackCount, itemEquipLoc, itemTexture:
    // nine values (`0x48e070` ends `mov eax,0x9`); later clients insert `itemLevel` at 4. The name
    // is the bare template name, where the reference appends the random suffix (`0x5d8b00`); the
    // link is the item string (`0x48e1c9`, literal `0x842d4c`), not a hyperlink; the level is the
    // required one (`[record+0x3c]`; `ItemLevel` at `+0x38` is never pushed). An uncached
    // template answers no values and asks nothing: the lookup passes no callback (`0x48e172`), so
    // the cache (`0x55ba30`) returns null without a query (`0x55baac`), sending the binding to
    // `xor eax,eax; ret`. The miss is recorded as a read, which the app answers from its cache.
    lua.globals().set(
        "GetItemInfo",
        lua.create_function(|lua, arg: Value| {
            let (id, enchant, random_property, suffix) = parse_item_arg(&arg)?;
            let Ok(item_id) = u32::try_from(id) else {
                return Ok(MultiValue::new()); // no record holds a negative or past-u32 id
            };
            let view = {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                let v = model.item_templates.get(&item_id).cloned();
                if v.is_none() && item_id != 0 {
                    model.item_stat_reads.insert(item_id);
                }
                v
            };
            let Some(v) = view else {
                return Ok(MultiValue::new());
            };
            let str_or_empty = |s: &Option<String>| s.clone().unwrap_or_default();
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&v.name)?),
                Value::String(lua.create_string(format!(
                    "item:{item_id}:{enchant}:{random_property}:{suffix}"
                ))?),
                Value::Integer(i64::from(v.quality)),
                Value::Integer(i64::from(v.required_level)),
                Value::String(lua.create_string(str_or_empty(&v.item_type))?),
                Value::String(lua.create_string(str_or_empty(&v.item_sub_type))?),
                Value::Integer(i64::from(v.stackable)),
                Value::String(lua.create_string(equip_loc_token(v.inventory_type))?),
                // Deviation: nil for a display row with no icon, where the reference answers the
                // unloadable `Interface\Icons\`, because `SetTexture` shows no icon for either.
                match &v.icon {
                    Some(icon) => Value::String(lua.create_string(icon)?),
                    None => Value::Nil,
                },
            ]))
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{item_usable, ItemTemplateView, PlayerReqState};
    use crate::script::UiScript;

    #[test]
    fn item_usable_mirrors_the_gate_legs() {
        let base_item = ItemTemplateView {
            allowable_class: -1,
            allowable_race: -1,
            ..Default::default()
        };
        let base_req = PlayerReqState {
            level: 4,
            class_id: 1, // warrior
            race_id: 2,  // orc
            ..Default::default()
        };
        let knows_none = |_: u32| false;
        assert!(item_usable(&base_item, &base_req, knows_none));

        // Level 5 required: level 4 fails, 5 passes (`jg`).
        let mut v = base_item.clone();
        v.required_level = 5;
        assert!(!item_usable(&v, &base_req, knows_none));
        let mut req = base_req.clone();
        req.level = 5;
        assert!(item_usable(&v, &req, knows_none));

        let mut v = base_item.clone();
        v.allowable_class = 1 << 3; // rogue-only (class 4)
        assert!(!item_usable(&v, &base_req, knows_none));
        let mut v = base_item.clone();
        v.allowable_race = 1 << 0; // human-only
        assert!(!item_usable(&v, &base_req, knows_none));

        // No alternate walk: a 2H axe fails with only the 1H bit set.
        let mut v = base_item.clone();
        (v.class, v.subclass) = (2, 1); // Two-Handed Axe
        let mut req = base_req.clone();
        req.proficiency.insert(2, 1 << 0); // knows One-Handed Axes only
        assert!(!item_usable(&v, &req, knows_none));
        req.proficiency.insert(2, 1 << 1);
        assert!(item_usable(&v, &req, knows_none));
        // No mask for the class: the leg never fires.
        let mut v = base_item.clone();
        (v.class, v.subclass) = (0, 0);
        assert!(item_usable(&v, &base_req, knows_none));

        // An unknown skill fails even at rank 0.
        let mut v = base_item.clone();
        v.required_skill = 164; // Blacksmithing
        assert!(!item_usable(&v, &base_req, knows_none));
        let mut req = base_req.clone();
        req.skills.insert(164, 0);
        assert!(item_usable(&v, &req, knows_none));
        v.required_skill_rank = 100;
        assert!(!item_usable(&v, &req, knows_none));
        req.skills.insert(164, 100);
        assert!(item_usable(&v, &req, knows_none));

        let mut v = base_item.clone();
        v.required_spell = 9787; // Weaponsmith
        assert!(!item_usable(&v, &base_req, knows_none));
        assert!(item_usable(&v, &base_req, |id| id == 9787));

        let mut v = base_item.clone();
        v.required_honor_rank = 3;
        assert!(!item_usable(&v, &base_req, knows_none));
        let mut req = base_req.clone();
        req.honor_rank = 3;
        assert!(item_usable(&v, &req, knows_none));

        let mut v = base_item.clone();
        v.required_city_rank = 1;
        assert!(!item_usable(&v, &base_req, knows_none));

        // An unknown faction is rank 0.
        let mut v = base_item.clone();
        (v.required_rep_faction, v.required_rep_rank) = (87, 5);
        assert!(!item_usable(&v, &base_req, knows_none));
        let mut req = base_req.clone();
        req.rep_ranks.insert(87, 5);
        assert!(item_usable(&v, &req, knows_none));
        let mut v = base_item.clone();
        (v.required_rep_faction, v.required_rep_rank) = (87, 0);
        assert!(item_usable(&v, &base_req, knows_none));

        // An unpushed state (level 0) declines to judge.
        let mut v = base_item.clone();
        v.required_level = 60;
        assert!(item_usable(&v, &PlayerReqState::default(), knows_none));
    }

    /// A miss records a read, never an ask: `GetItemInfo`'s lookup passes no callback
    /// (`0x48e172`).
    #[test]
    fn miss_records_a_read_and_push_serves_the_stats() {
        let mut s = UiScript::new().unwrap();
        assert!(s.eval::<bool>("return GetItemInfo(25) == nil").unwrap());
        assert_eq!(s.take_item_stat_reads(), vec![25]);
        assert!(s.take_item_stat_asks().is_empty(), "no ask of the server");

        s.set_item_template(
            25,
            ItemTemplateView {
                name: "Worn Shortsword".into(),
                quality: 1,
                inventory_type: 21,
                class: 2,
                subclass: 7,
                damages: vec![(1.0, 3.0, 0)],
                delay_ms: 1900,
                ..Default::default()
            },
        );
        let (name, quality): (String, i64) = s
            .eval("local n, _, q = GetItemInfo(25) return n, q")
            .unwrap();
        assert_eq!((name.as_str(), quality), ("Worn Shortsword", 1));
        assert!(s.take_item_stat_reads().is_empty(), "push cleared the read");
        // Id 0 records no read.
        assert!(s.eval::<bool>("return GetItemInfo(0) == nil").unwrap());
        assert!(s.take_item_stat_reads().is_empty());
    }
}

#[cfg(test)]
mod get_item_info_tests {
    use super::{equip_loc_token, ItemTemplateView};
    use crate::script::UiScript;

    /// Worn Shortsword, item 25, as vmangos `item_template` holds it (item level 2).
    fn worn_shortsword() -> ItemTemplateView {
        ItemTemplateView {
            name: "Worn Shortsword".into(),
            quality: 1,
            class: 2,
            subclass: 7,
            inventory_type: 21,
            item_type: Some("Weapon".into()),
            // VerboseName, not the tooltip cell's "Sword" (`0x48e311`).
            item_sub_type: Some("One-Handed Swords".into()),
            // Required level 1 against item level 2 makes position 4 falsifiable.
            required_level: 1,
            stackable: 1,
            max_count: 0,
            icon: Some("Interface\\Icons\\INV_Sword_04".into()),
            ..Default::default()
        }
    }

    /// The arity catches a later client's shape, which inserts `itemLevel` at position 4.
    #[test]
    fn get_item_info_returns_the_1_12_nine_value_shape() {
        let mut s = UiScript::new().unwrap();
        s.set_item_template(25, worn_shortsword());

        assert_eq!(
            s.arity("GetItemInfo(25)").unwrap(),
            9,
            "1.12 returns nine values (`mov eax,0x9` at 0x48e303) — a tenth means the modern shape"
        );

        let (name, link, quality, min_level, ty, sub_ty, stack, equip, texture): (
            String,
            String,
            i64,
            i64,
            String,
            String,
            i64,
            String,
            Option<String>,
        ) = s
            .eval("local a,b,c,d,e,f,g,h,i = GetItemInfo(25) return a,b,c,d,e,f,g,h,i")
            .unwrap();
        assert_eq!(name, "Worn Shortsword");
        // The item string (`0x842d4c`); a number argument leaves the other fields 0.
        assert_eq!(link, "item:25:0:0:0");
        assert_eq!(quality, 1);
        assert_eq!(min_level, 1);
        assert_eq!(ty, "Weapon");
        assert_eq!(sub_ty, "One-Handed Swords");
        assert_eq!(stack, 1);
        assert_eq!(equip, "INVTYPE_WEAPONMAINHAND");
        assert_eq!(texture.as_deref(), Some("Interface\\Icons\\INV_Sword_04"));
    }

    /// Ashbringer, item 13262: required level 60, item level 76 in vmangos `item_template`.
    #[test]
    fn position_four_is_the_required_level_not_the_item_level() {
        let mut s = UiScript::new().unwrap();
        s.set_item_template(
            13262,
            ItemTemplateView {
                name: "Ashbringer".into(),
                quality: 5,
                class: 2,
                subclass: 8,
                inventory_type: 17,
                item_type: Some("Weapon".into()),
                item_sub_type: Some("Two-Handed Swords".into()),
                required_level: 60, // item level 76 is never returned
                max_count: 1,
                stackable: 1,
                ..Default::default()
            },
        );
        assert_eq!(
            s.eval::<i64>("local _, _, _, minLevel = GetItemInfo(13262) return minLevel")
                .unwrap(),
            60,
            "position 4 is RequiredLevel ([record+0x3c]); ItemLevel lives at +0x38 and is not pushed"
        );
        assert_eq!(
            s.eval::<String>("local _, _, _, _, itemType = GetItemInfo(13262) return itemType")
                .unwrap(),
            "Weapon",
            "position 5 is itemType — a modern shape would put a NUMBER (itemMinLevel) here"
        );
        assert_eq!(
            s.eval::<String>(
                "local _, _, _, _, _, _, _, equipLoc = GetItemInfo(13262) return equipLoc"
            )
            .unwrap(),
            "INVTYPE_2HWEAPON"
        );
    }

    /// Linen Cloth, item 2589: `max_count 0, stackable 20` in vmangos `item_template`.
    #[test]
    fn position_seven_is_the_stack_size_not_the_unique_cap() {
        let mut s = UiScript::new().unwrap();
        s.set_item_template(
            2589,
            ItemTemplateView {
                name: "Linen Cloth".into(),
                quality: 1,
                class: 7,
                subclass: 0,
                item_type: Some("Trade Goods".into()),
                item_sub_type: Some("Trade Goods".into()),
                max_count: 0,
                stackable: 20,
                ..Default::default()
            },
        );
        let (stack, equip): (i64, String) = s
            .eval("local _,_,_,_,_,_,g,h = GetItemInfo(2589) return g,h")
            .unwrap();
        assert_eq!(stack, 20, "Stackable (+0x60), never MaxCount (+0x5c)");
        // InventoryType 0 answers "", not nil.
        assert_eq!(equip, "");
    }

    #[test]
    fn an_uncached_id_returns_nothing_and_records_a_read() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(
            s.arity("GetItemInfo(2589)").unwrap(),
            0,
            "an unseen template returns NO values"
        );
        assert!(s.eval::<bool>("return (GetItemInfo(2589)) == nil").unwrap());
        assert_eq!(
            s.take_item_stat_reads(),
            vec![2589],
            "the read was recorded"
        );
        assert!(s.take_item_stat_asks().is_empty(), "and no ask");

        s.set_item_template(
            2589,
            ItemTemplateView {
                name: "Linen Cloth".into(),
                stackable: 20,
                ..Default::default()
            },
        );
        assert_eq!(
            s.eval::<String>("return (GetItemInfo(2589))").unwrap(),
            "Linen Cloth",
            "the app's push answers the next read"
        );
        assert!(s.take_item_stat_asks().is_empty(), "the push cleared it");

        // Id 0 and a negative id record no ask.
        assert_eq!(s.arity("GetItemInfo(0)").unwrap(), 0);
        assert_eq!(s.arity("GetItemInfo(-5)").unwrap(), 0);
        assert!(s.take_item_stat_asks().is_empty());
    }

    #[test]
    fn the_argument_forms_are_the_references_own() {
        let mut s = UiScript::new().unwrap();
        s.set_item_template(25, worn_shortsword());

        for arg in ["25", "\"25\"", "25.7"] {
            assert_eq!(
                s.eval::<String>(&format!("return (GetItemInfo({arg}))"))
                    .unwrap(),
                "Worn Shortsword",
                "argument {arg}"
            );
        }

        // An item string's other fields echo into return 2 verbatim.
        let (name, link): (String, String) = s
            .eval("local a,b = GetItemInfo(\"item:25:2564:7:0\") return a,b")
            .unwrap();
        assert_eq!(name, "Worn Shortsword");
        assert_eq!(link, "item:25:2564:7:0");
        // A case-insensitive prefix, and a short string still parses.
        assert_eq!(
            s.eval::<String>("local _, link = GetItemInfo(\"ITEM:25\") return link")
                .unwrap(),
            "item:25:0:0:0"
        );

        // A bare name and a full hyperlink are id 0: no values and no ask.
        for arg in [
            "\"Worn Shortsword\"",
            "\"|cffffffff|Hitem:25:0:0:0|h[Worn Shortsword]|h|r\"",
            "\"\"",
        ] {
            assert_eq!(
                s.arity(&format!("GetItemInfo({arg})")).unwrap(),
                0,
                "argument {arg} resolves to item id 0"
            );
        }
        assert!(s.take_item_stat_asks().is_empty());

        // Neither a number nor a string raises the usage error (`0x842d24`).
        let err = s
            .eval::<mlua::Value>("return GetItemInfo(nil)")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("Usage: GetItemInfo(itemID|\"itemlink\")"),
            "expected the reference's usage error, got {err}"
        );
    }

    #[test]
    fn the_equip_loc_tokens_are_the_binarys_table() {
        assert_eq!(equip_loc_token(0), "", "index 0 is the shared \"\"");
        assert_eq!(equip_loc_token(1), "INVTYPE_HEAD");
        assert_eq!(equip_loc_token(3), "INVTYPE_SHOULDER", "singular");
        assert_eq!(equip_loc_token(9), "INVTYPE_WRIST", "singular");
        assert_eq!(equip_loc_token(10), "INVTYPE_HAND", "singular");
        assert_eq!(equip_loc_token(17), "INVTYPE_2HWEAPON");
        assert_eq!(equip_loc_token(20), "INVTYPE_ROBE");
        // The four with no `GlobalStrings.lua` entry.
        assert_eq!(equip_loc_token(24), "INVTYPE_AMMO");
        assert_eq!(equip_loc_token(25), "INVTYPE_THROWN");
        assert_eq!(equip_loc_token(26), "INVTYPE_RANGEDRIGHT");
        assert_eq!(equip_loc_token(27), "INVTYPE_QUIVER");
        assert_eq!(equip_loc_token(28), "INVTYPE_RELIC", "the last real token");
        assert_eq!(equip_loc_token(29), "", "the array's trailing \"\"");
        assert_eq!(equip_loc_token(9999), "", "past the array");
    }
}

#[cfg(test)]
mod quality_color_tests {
    use crate::script::UiScript;

    #[test]
    fn get_item_quality_color_is_the_references_own_table() {
        let s = UiScript::new().unwrap();
        let hex = |q: i64| {
            s.eval::<String>(&format!(
                "local r,g,b,h = GetItemQualityColor({q}) return h"
            ))
            .unwrap()
        };
        for (q, want) in [
            (0, "|cff9d9d9d"),
            (1, "|cffffffff"),
            (2, "|cff1eff00"),
            (3, "|cff0070dd"),
            (4, "|cffa335ee"),
            (5, "|cffff8000"),
            (6, "|cffe6cc80"),
        ] {
            assert_eq!(hex(q), want, "quality {q}");
        }

        // The floats are byte / 255: Epic's red is 0xa3.
        let r = s
            .eval::<f64>("local r = GetItemQualityColor(4) return r")
            .unwrap();
        assert!((r - 163.0 / 255.0).abs() < 1e-9, "epic red was {r}");

        // 7 and up answer Common (`0x52ad70`).
        assert_eq!(hex(7), "|cffffffff");
        assert_eq!(hex(99), "|cffffffff");
        // The compare is unsigned, so a negative answers Common too.
        assert_eq!(hex(-1), "|cffffffff");
        assert_eq!(hex(-99), "|cffffffff");
    }
}
