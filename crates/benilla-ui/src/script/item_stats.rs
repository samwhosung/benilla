//! The **shared item-template store** — one `item id → ItemTemplateView` table every item hover
//! renders through (decision 0274 P1). In the real client every item tooltip is the same C++
//! renderer (`0x52b650`, behind 8 of the 9 `Set*Item` bindings) over the same item-template cache;
//! benilla mirrors that with
//! one engine store the app feeds from its ask-once `ITEM_QUERY` template cache, and one engine
//! renderer ([`super::tooltip_item`]).
//!
//! Fill flow: the app **pushes** every item template the moment it lands in its cache
//! ([`super::UiScript::set_item_template`]) — arrival-driven, so a first hover of an item whose
//! name is already on screen always hits. A renderer read of an id the app never resolved
//! additionally records the id ([`super::UiScript::take_item_stat_asks`] drains them), which
//! makes the app send `CMSG_ITEM_QUERY` and push when the answer arrives — the real client's
//! uncached-item early-out (cleared tooltip + query; the hover's re-enter loop repaints on
//! arrival).
//!
//! The view carries the template's tooltip-relevant fields plus the strings only the app can
//! resolve (skill/faction names from the DBC catalogs, trigger-spell display text) — the engine
//! renders lines, it never reads DBCs.

use std::collections::HashMap;

use mlua::{Lua, MultiValue, Value};

use super::Model;

/// An item template's tooltip view (decision 0274 P1) — the fields the line law consumes, in
/// wire terms (vmangos `SMSG_ITEM_QUERY_SINGLE_RESPONSE`), plus app-resolved display strings.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ItemTemplateView {
    /// The name line (quality-colored).
    pub name: String,
    pub quality: u32,
    /// Item class/subclass (the slot line's right column) + the equip slot (its left).
    pub class: u32,
    pub subclass: u32,
    pub inventory_type: u32,
    /// `GetItemInfo`'s `itemType`: what [`Self::class`] is **called** — "Weapon", "Container",
    /// "Trade Goods" — app-resolved from `ItemClass.dbc` (byte-verified: `0x48e070` pushes
    /// `[classRow + 4*locale + 0xc]`). `None` for a class with no row, which the reference
    /// renders as the empty string.
    pub item_type: Option<String>,
    /// `GetItemInfo`'s `itemSubType`: what `(class, subclass)` is **called** — app-resolved from
    /// `ItemSubClass.dbc`, **VerboseName first with a DisplayName fallback**
    /// ([`benilla_formats::ItemSubClassCatalog::name`]), which is the binding's own two-step at
    /// `0x48e311`: read `[row + 4*locale + 0x4c]` and use it unless its first byte is 0, else
    /// `[row + 4*locale + 0x28]`.
    ///
    /// **This is a different spelling from the tooltip's type cell**, which reads `+0x28`
    /// (DisplayName) only — a one-handed sword is "One-Handed Swords" here and "Sword" there.
    pub item_sub_type: Option<String>,
    /// What `(class, subclass)` is called on the TOOLTIP's own type cell — `ItemSubClass.dbc`'s
    /// **DisplayName** alone (`row + 4*locale + 0x28`, column 10: the read the builder makes at
    /// `0x52c0xx` off the `0xc0db90` row cache), which is the SINGULAR spelling: "Sword", not
    /// [`Self::item_sub_type`]'s "One-Handed Swords". App-resolved, because the engine reads no
    /// DBCs — and it is the same string the bag line's `CONTAINER_SLOTS` second hole takes.
    ///
    /// This used to be a hand-typed `(class, subclass) → &str` table in the renderer, which is
    /// the shape decision 2080 caught one file over: a stale copy of data the catalog already
    /// held, missing every container family outright.
    pub sub_class_display: Option<String>,
    /// The alternate subclass whose proficiency also permits use (ItemSubClass.dbc
    /// prerequisite/postrequisite, app-resolved with the builder's sentinel walk: prerequisite
    /// wins, postrequisite only when prerequisite is −1). A weapon missing its own mask bit but
    /// holding the alternate's reds the SLOT cell instead of the type cell.
    pub proficiency_alt: Option<u32>,
    /// ItemSubClass displayFlags bit 0 (app-resolved): the type cell never prints — the
    /// "Miscellaneous" family (rings, trinkets, shirts, consumables, recipes).
    pub hide_subclass: bool,
    /// Template flags — bit `0x2` = conjured ("Conjured Item").
    pub flags: u32,
    /// Bonding: 1 = binds on pickup, 2 = on equip, 3 = on use, 4/5 = quest item.
    pub bonding: u32,
    /// `MaxCount` — 1 = "Unique", N > 1 = "Unique (N)", 0 = no line. **Not the stack size**; see
    /// [`Self::stackable`].
    pub max_count: u32,
    /// `Stackable` — the biggest stack one slot can hold (1 = doesn't stack). `GetItemInfo`'s
    /// `itemStackCount`, read from the cache record's `+0x60` at `0x48e28b` — the dword *after*
    /// `MaxCount` at `+0x5c`. Linen Cloth is `maxcount 0, stackable 20`, so the two are not
    /// interchangeable in either direction.
    pub stackable: u32,
    /// Nonzero = "This Item Begins a Quest".
    pub start_quest: u32,
    /// Container size — "N Slot Bag".
    pub container_slots: u32,
    /// Stat mods `(type, value)` in wire order (types: 0 mana, 1 health, 3 agi, 4 str, 5 int,
    /// 6 spi, 7 stam — the `ITEM_MOD_*` GlobalStrings family).
    pub stats: Vec<(u32, i32)>,
    /// Damage blocks `(min, max, school)` in wire order; school 0 physical, 1..6 =
    /// Holy/Fire/Nature/Frost/Shadow/Arcane.
    pub damages: Vec<(f32, f32, u32)>,
    pub delay_ms: u32,
    pub armor: u32,
    pub block: u32,
    /// Holy..Arcane (armor is its own field), the "+N X Resistance" lines.
    pub resistances: [i32; 6],
    /// "Durability N / N" (a template hover shows full).
    pub max_durability: u32,
    /// "Requires Level N" — printed only for N > 1 (the real builder's `0x52d2cf` gate); red
    /// when the player is lower.
    pub required_level: u32,
    /// Class/race masks; `<= 0` = everyone (no line). Red when the player's bit is absent.
    pub allowable_class: i32,
    pub allowable_race: i32,
    /// Skill requirement: the SkillLine id + rank, with the display name app-resolved
    /// (`SkillLine.dbc`). `required_skill_name = None` = no skill line.
    pub required_skill: u32,
    pub required_skill_rank: u32,
    pub required_skill_name: Option<String>,
    /// Spell requirement (nonzero = "Requires <name>") — red when the spellbook doesn't know it.
    pub required_spell: u32,
    pub required_spell_name: Option<String>,
    /// `RequiredHonorRank` — no tooltip line in 1.12, but the item-usable gate (`0x5ea930`)
    /// compares it against the player's highest honor rank; the merchant list reds on it.
    pub required_honor_rank: u32,
    /// `RequiredCityRank` — usable-gate only, like the honor rank. Vanilla data ships no nonzero
    /// value and vmangos never writes the `PVP_MEDALS` bits the client would test, so a nonzero
    /// requirement can only fail (see [`item_usable`]).
    pub required_city_rank: u32,
    /// Reputation requirement, app-resolved to "Requires <Faction> - <Standing>"; the raw
    /// faction id + rank ride along for the red check against [`PlayerReqState::rep_ranks`].
    pub required_rep_line: Option<String>,
    pub required_rep_faction: u32,
    pub required_rep_rank: u32,
    /// Trigger-spell lines `(trigger, spell id, display text)` in wire order: trigger 0/5 =
    /// "Use:", 1 = "Equip:", 2 = "Chance on hit:", 6 = a taught spell (the "Already known" red
    /// check, no green line). The text is app-resolved (the spell's name in P1; its substituted
    /// description in P2) — green lines.
    pub spell_triggers: Vec<(u32, u32, String)>,
    /// `LockID` — nonzero prints the red "Locked" line (the key-item sub-line joins with the
    /// Lock.dbc resolve, the GO-locks follow-up).
    pub lock_id: u32,
    /// "N Charge(s)" — the app-resolved count for the first spell slot that survives the real
    /// builder's charge gate (`0x52db51`: a slot whose value is 0 or the `-1` consume-on-use
    /// sentinel prints nothing; else `abs`). 0 = no line.
    pub charges: i32,
    /// The yellow quoted flavor text (wrapped).
    pub description: String,
    /// Nonzero = "<Right Click to Read>" (green).
    pub page_text: u32,
    /// Copper — the merchant-open money row (the engine fires `OnTooltipAddMoney`), and
    /// `ITEM_UNSELLABLE` when 0 in a sell context.
    pub sell_price: u32,
    /// `itemset` — nonzero renders the SET block ([`ItemSetView`], asked once by set id).
    pub item_set: u32,
    /// The inventory icon as a ready `Interface\Icons\…` path — app-resolved through
    /// `ItemDisplayInfo.dbc` off the template's `display_info_id`, the same lookup the bag slots
    /// use. `GetItemInfo`'s `itemTexture`; the reference builds the identical string at
    /// `0x48e2dd` (`"%s%s%s"` over the icon directory, `"\\"`, and the row's icon name).
    /// `None` on the ~26% of display rows that carry no icon.
    pub icon: Option<String>,
    /// `RandomProperty` (template `+0x1b8`) — the item CAN roll a "… of the Bear" suffix. Its one
    /// consumer is the enchant family's third arm: with no instance to read a roll from, the
    /// tooltip prints the `<Random enchantment>` placeholder instead of any per-slot line
    /// (`0x52cc33`).
    pub random_property: u32,
}

/// The player state the red-line law compares against (decision 0274 P1): pushed by the app
/// whenever it changes. Level and class also ride the `"player"` unit feed; this carries the
/// pieces that don't (the class/race IDS as mask bits, and the skill ranks).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlayerReqState {
    pub level: u32,
    /// Class id (1 warrior … 11 druid) — the `allowable_class` mask bit is `1 << (id-1)`.
    pub class_id: u32,
    /// Race id (1 human … 8 troll) — the `allowable_race` mask bit is `1 << (id-1)`.
    pub race_id: u32,
    /// SkillLine id → current rank, for "Requires <skill> (N)" checks.
    pub skills: HashMap<u32, u32>,
    /// Item class (2 weapons / 4 armor) → allowed-subclass bitmask (`SMSG_SET_PROFICIENCY`,
    /// the client's `0xc4d4a0[class]` store). The slot-line proficiency red: a class WITH an
    /// entry here reds when the item's `1 << subclass` bit is absent; a class with no entry
    /// (consumables etc.) never reds.
    pub proficiency: HashMap<u32, u32>,
    /// Faction id → the player's reputation rank (0 hated … 7 exalted; DBC base + wire
    /// standing, app-ranked) — the "Requires <Faction> - <Standing>" red check.
    pub rep_ranks: HashMap<u32, u8>,
    /// Whether the spellbook holds an effect-40 (SPELL_EFFECT_DUAL_WIELD) spell — the client's
    /// `0xc4d770` global (stored on learn, cleared on unlearn; reader `0x5eab70`). An off-hand
    /// weapon (InventoryType 22) reds its SLOT cell without it.
    pub can_dual_wield: bool,
    /// The player's **highest lifetime honor rank** (`PLAYER_FIELD_BYTES` byte 3) — the
    /// usable-gate's `RequiredHonorRank` comparand.
    pub honor_rank: u8,
}

/// [`item_usable`] for an item the caller knows only by **entry** — the shape every window seam
/// wants, since a row carries an id and the engine holds the templates.
///
/// Two conventions are folded in here rather than re-derived per caller: entry `0` (a row with no
/// item at all) is usable, and a template the app has not answered for yet is usable, which is the
/// getter's own null-record skip. It was four identical private copies across the merchant, mail,
/// trade and auction seams before it was one — three had the `item_id == 0` leg and the fourth
/// reached the same answer through `is_none_or`, which is exactly the kind of agreement that stops
/// being reliable the moment someone edits one of them.
pub(super) fn item_usable_by_id(model: &super::Model, item_id: u32) -> bool {
    item_id == 0
        || model.item_templates.get(&item_id).is_none_or(|v| {
            item_usable(v, &model.player_req, |id| {
                model.spellbook.slots.iter().any(|s| s.spell_id == id)
            })
        })
}
/// The client's item-usable predicate `0x5ea930(player; itemCacheRecord, &err)`. Both merchant
/// getters call it (`GetMerchantItemInfo` `0x4fb2a3`, `GetBuybackItemInfo` `0x4fb4f7`) and push
/// `1`/`nil` as `isUsable`; the FrameXML reds the row on `nil`. The legs, in the binary's order:
///
/// 1. `requiredLevel > player level` → unusable.
/// 2. class mask: `allowableClass & 1<<(classId−1)` clear → unusable (−1 = every bit set).
/// 3. race mask: same test against `allowableRace`.
/// 4. proficiency: a mask exists for the item class AND the item's **own** subclass bit is clear
///    → unusable. NO ItemSubClass alternate walk here — the alternate only chooses which tooltip
///    CELL reds; the usable gate is the raw bit.
/// 5. `requiredSkill`: unknown skill → unusable; known → `value + permBonus ≥ requiredSkillRank`.
/// 6. `requiredSpell`: not in the spellbook → unusable.
/// 7. `requiredHonorRank`: player's highest honor rank (`PLAYER_FIELD_BYTES` byte 3) short →
///    unusable.
/// 8. `requiredCityRank`: tests `PLAYER_FIELD_PVP_MEDALS & 1<<(rank−1)` — vanilla never writes
///    the medals field and ships no city-rank items, so a nonzero requirement always fails;
///    mirrored as the constant result rather than plumbing a dead field.
/// 9. reputation: player standing ≥ the required rank's threshold (`0x4d6370` +
///    the `0x80928c` threshold table) — equivalently rank ≥ requiredRepRank, the tooltip red's
///    exact compare; an unknown faction counts as rank 0.
///
/// A template the cache hasn't answered yet is USABLE (the getter skips the call on a null
/// record — `0x4fb298`); the engine analog is an unpushed [`PlayerReqState`] (level 0, a state
/// the real client can't reach), which also declines to judge.
pub fn item_usable(
    v: &ItemTemplateView,
    req: &PlayerReqState,
    knows_spell: impl Fn(u32) -> bool,
) -> bool {
    if req.level == 0 {
        return true;
    }
    if v.required_level > req.level {
        return false;
    }
    if req.class_id == 0 || v.allowable_class as u32 & (1 << (req.class_id - 1)) == 0 {
        return false;
    }
    if req.race_id == 0 || v.allowable_race as u32 & (1 << (req.race_id - 1)) == 0 {
        return false;
    }
    if let Some(&mask) = req.proficiency.get(&v.class) {
        if mask & (1 << v.subclass) == 0 {
            return false;
        }
    }
    if v.required_skill != 0 {
        match req.skills.get(&v.required_skill) {
            None => return false,
            Some(&val) => {
                if val < v.required_skill_rank {
                    return false;
                }
            }
        }
    }
    if v.required_spell != 0 && !knows_spell(v.required_spell) {
        return false;
    }
    if v.required_honor_rank != 0 && u32::from(req.honor_rank) < v.required_honor_rank {
        return false;
    }
    if v.required_city_rank != 0 {
        return false;
    }
    if v.required_rep_faction != 0 {
        let rank = req
            .rep_ranks
            .get(&v.required_rep_faction)
            .copied()
            .unwrap_or(0);
        if u32::from(rank) < v.required_rep_rank {
            return false;
        }
    }
    true
}

/// An item set's tooltip view (`0x854b1c`'s SET block), app-resolved: the ItemSet.dbc row with
/// member item NAMES joined from the template cache (a `None` name = the member's template is
/// still in flight — its line waits; the app re-pushes as answers land) and the threshold
/// bonuses' TEXT ($-substituted spell descriptions). The engine supplies the live half: the
/// owned/equipped counts off its own inventory slots.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ItemSetView {
    pub name: String,
    /// `(item id, resolved name)` per member, DBC order.
    pub members: Vec<(u32, Option<String>)>,
    /// `(required equipped count, bonus text)` per bonus, in the DBC's stored slot order —
    /// the renderer sorts threshold-ascending at print time, like the builder's qsort
    /// (`0x52e5c0`; The Gladiator ships 3,2,5,4 and prints 2,3,4,5).
    pub bonuses: Vec<(u32, String)>,
    /// The set-level skill requirement ("Requires <skill> (N)"), app-named; 0/None = no line.
    pub required_skill: u32,
    pub required_skill_rank: u32,
    pub required_skill_name: Option<String>,
}

impl super::UiScript {
    /// Store (or replace) an item's template view — the app's push half of the ask-once flow.
    pub fn set_item_template(&mut self, item_id: u32, view: ItemTemplateView) {
        let mut model = self.model_mut();
        model.item_stat_asks.remove(&item_id);
        model.item_templates.insert(item_id, view);
    }

    /// Drain the ids the renderer asked for that the store didn't have.
    /// Ask the app for the templates of `ids` the store does not hold yet — the trade-skill and
    /// craft feeds' pre-ask (1973): the client has every product's and reagent's template cached
    /// by the time its list shows, and the link verbs never query, so the feeds ask on the app's
    /// behalf when the list lands and the verbs read the answers.
    pub fn ask_item_templates(&mut self, ids: impl IntoIterator<Item = u32>) {
        let mut model = self.model_mut();
        for id in ids {
            if id != 0 && !model.item_templates.contains_key(&id) {
                model.item_stat_asks.insert(id);
            }
        }
    }

    pub fn take_item_stat_asks(&mut self) -> Vec<u32> {
        self.model_mut().item_stat_asks.drain().collect()
    }

    /// Store (or replace) a set's view — the SET block's push half (re-pushed as member names
    /// resolve).
    pub fn set_item_set(&mut self, set_id: u32, view: ItemSetView) {
        let mut model = self.model_mut();
        model.item_set_asks.remove(&set_id);
        model.item_sets.insert(set_id, view);
    }

    /// Drain the set ids the renderer asked for that the store didn't have.
    pub fn take_item_set_asks(&mut self) -> Vec<u32> {
        self.model_mut().item_set_asks.drain().collect()
    }

    /// Push the **whole** random-suffix table (`ItemRandomProperties` id → its resolved view) —
    /// once, at load. Not an ask-once store: see [`super::Model::random_properties`] for why the
    /// roll table is pushed whole where the template store is asked for.
    pub fn set_random_properties(
        &mut self,
        rows: std::collections::HashMap<u32, super::RandomPropertyView>,
    ) {
        self.model_mut().random_properties = rows;
    }

    /// Push the red-line law's player state (level/class/race/skills). Cheap to call on change.
    pub fn set_player_req_state(&mut self, state: PlayerReqState) {
        self.model_mut().player_req = state;
    }
}

/// Register the shared item-stats global (the P0 Lua stat-head read — still the merchant
/// sell-cursor's source and the compat surface while call sites finish moving to the engine
/// renderer; the tooltip itself no longer routes through it).
/// The 1.12 item-quality palette: the seven ARGB literals the static init `0x5291d0` writes into
/// the BGRA array at `0xc0d3c8`, and the parallel escape strings at `0x854124`.
///
/// `Script::GetItemQualityColor 0x48dfb0` reads `[quality*4 + 0xc0d3c8]`, takes `[+2]=R`,
/// `[+1]=G`, `[+0]=B`, multiplies each by `1/255`, and returns them with the escape string.
const QUALITY_COLORS: [(u8, u8, u8, &str); 7] = [
    (0x9d, 0x9d, 0x9d, "|cff9d9d9d"), // 0 Poor
    (0xff, 0xff, 0xff, "|cffffffff"), // 1 Common
    (0x1e, 0xff, 0x00, "|cff1eff00"), // 2 Uncommon
    (0x00, 0x70, 0xdd, "|cff0070dd"), // 3 Rare
    (0xa3, 0x35, 0xee, "|cffa335ee"), // 4 Epic
    (0xff, 0x80, 0x00, "|cffff8000"), // 5 Legendary
    (0xe6, 0xcc, 0x80, "|cffe6cc80"), // 6 Artifact
];

/// The client's item-link builder `0x52adb0` as the trade-skill and craft link verbs call it
/// (1973): `|c<rrggbb>|Hitem:<id>:0:0:0|h[<name>]|h|r`
/// — the three tokens literal zeros at those call sites, the colour from `0x52ad90`'s table where
/// `cmp ecx,7; jb` is UNSIGNED, so a quality of 7 or more, or a negative one, selects index 1
/// (white): a fixed fallback, not a clamp. The `""`-suffix arm is dead, so every link ends `|h|r`.
pub(super) fn item_link(item_id: u32, name: &str, quality: u32) -> String {
    let hex = QUALITY_COLORS
        .get(quality as usize)
        .unwrap_or(&QUALITY_COLORS[1])
        .3;
    format!("{hex}|Hitem:{item_id}:0:0:0|h[{name}]|h|r")
}

/// `InventoryType` → the `INVTYPE_*` token `GetItemInfo` returns as `itemEquipLoc`.
///
/// **The reference's own table, read out of the binary**: `0x48e29b` does
/// `mov eax,[record+0x2c]` (the inventory type) then `mov edx,[eax*4 + 0x83ddb0]` and pushes the
/// string — an unguarded index into a 30-pointer array at `0x83ddb0`. Its entries, decoded from
/// the PE image, are exactly the 28 tokens below at 1..=28, with index **0 and 29 both pointing at
/// the shared empty string `0x882748`**. So a non-equippable item answers `""`, not nil: an addon
/// doing `getglobal(itemEquipLoc)` gets nil either way, but one doing `itemEquipLoc == ""` — or
/// concatenating it — sees what the real client shows.
///
/// The spellings are the client's, and four of them (`AMMO`, `THROWN`, `RANGEDRIGHT`, `QUIVER`)
/// have **no matching GlobalString** in the shipped 1.12 `GlobalStrings.lua`, which defines only
/// 24 `INVTYPE_*` entries. That asymmetry is the reference's, not ours: the binding hands back a
/// token the FrameXML cannot localize. Note also `SHOULDER`/`WRIST`/`HAND` are singular here where
/// our numeric equip-slot map's comments say `SHOULDERS`/`WRISTS`/`HANDS`.
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
    // The reference does not bounds-check (the value always comes off its own cache record); we
    // do, because a Lua-reachable path must not depend on that. 29 is the array's own trailing "".
    TOKENS.get(inventory_type as usize).copied().unwrap_or("")
}

/// The reference's `atoi` (`0x64ac60`), which is what parses each field of an `item:` string:
/// an optional leading `-`, then decimal digits, stopping at the first byte that is not one.
/// **No leading-whitespace skip, no `+`, no `0x`** — and no overflow check either, but a saturating
/// accumulate is the sane read of a value that can only come from a script.
fn reference_atoi(s: &str) -> i64 {
    let b = s.as_bytes();
    let (neg, mut i) = match b.first() {
        Some(b'-') => (true, 1),
        _ => (false, 0),
    };
    let mut n: i64 = 0;
    while let Some(&c) = b.get(i) {
        if !c.is_ascii_digit() {
            break;
        }
        n = n
            .saturating_mul(10)
            .saturating_add(i64::from(c - b'0'))
            .min(i64::from(u32::MAX));
        i += 1;
    }
    if neg {
        -n
    } else {
        n
    }
}

/// `GetItemInfo`'s argument, resolved to the four `item:` fields — **the reference's parser**
/// (`0x48e0a3`-`0x48e16d`), which is narrower than every later client's and narrower than the
/// binding's own usage string suggests:
///
/// 1. `lua_isnumber` (`0x6f34d0` — LUA_TNUMBER **or a string Lua coerces to one**) → truncate to
///    an int and that is the whole answer; enchant/random-property/suffix stay 0. This is the arm
///    `GetItemInfo(2589)` and `GetItemInfo(tostring(id))` both take.
/// 2. else `lua_isstring` (`0x6f3510`) → `strnicmp(s, "item:", 5)` (`0x64a4c0` → `0x414310`). On a
///    match, `atoi` the four colon-separated fields; **on a miss the id stays 0** and the caller
///    returns nothing.
/// 3. else raise `Usage: GetItemInfo(itemID|"itemlink")` — the literal at `0x842d24`, pushed
///    through `0x6f4940` (`luaL_where` + `lua_pushvfstring` + concat + `lua_error`: it raises).
///
/// **So a bare item NAME and a full `|cff…|Hitem:…|h[Name]|h|r` hyperlink both resolve to nothing**
/// on the real 1.12 client — neither begins with `item:`. The "itemlink" the usage string means is
/// the *item string*, which is also what the binding returns as its second value. Auctioneer builds
/// exactly that (`AucItemDB.lua:333`: `string.format("item:%s:%s:%s:0", …)`) before calling this.
fn parse_item_arg(v: &Value) -> mlua::Result<(i64, u32, u32, u32)> {
    // Arm 1 — a number, or a string Lua's own coercion reads as one.
    let as_number = match v {
        Value::Integer(i) => Some(*i as f64),
        Value::Number(n) => Some(*n),
        Value::String(s) => s.to_str().ok().and_then(|s| s.trim().parse::<f64>().ok()),
        _ => None,
    };
    if let Some(n) = as_number {
        return Ok((n as i64, 0, 0, 0));
    }
    // Arm 2 — a string. Anything that is not `item:`-prefixed leaves the id at 0.
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
    let mut fields = rest.splitn(4, ':').map(reference_atoi);
    let id = fields.next().unwrap_or(0);
    let mut next = || u32::try_from(fields.next().unwrap_or(0)).unwrap_or(0);
    Ok((id, next(), next(), next()))
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    // `GetItemQualityColor(quality)` → `r, g, b, escapeString`.
    //
    // A **C-registered** binding in the reference (`0x48dfb0`) and `function engine` in the
    // captured `_G`, which is why it belongs here rather than in `assets/ui`: the shipped
    // `UIParent.lua` *builds* `ITEM_QUALITY_COLORS` by calling it in a loop (`for i = -1, 6`), so
    // the table is FrameXML's and the verb underneath is the engine's. 23 corpus addons call it,
    // and three of our own XML files carry a private copy of the palette this replaces.
    //
    // **The clamp is the reference's, not a guess** — and it is ONE clamp, not two, because the
    // comparison is UNSIGNED. `0x52ad70`/`0x52ad90` both do `cmp ecx,7` / `jb`, so anything at or
    // above 7 selects index **1** (Common) — not 6, not an error — and a NEGATIVE quality takes the
    // same branch, because as a `u32` it is at or above 7 too. `GetItemQualityColor(-1)` is
    // therefore **white**, and so is `ITEM_QUALITY_COLORS[-1]`, the row `UIParent.lua`'s
    // `for i = -1, 6` builds.
    //
    // This corrects 1199, which called the negative arm "the one place this binding is deliberately
    // not bit-faithful … because the faithful answer is an out-of-bounds read" and answered Poor
    // (grey). There is no out-of-bounds read: the unsigned compare catches it first. The value
    // stopped being academic when the loot window went to the chain — `-1` is what
    // `GetLootSlotInfo` answers for a row whose item template is not cached, and that row's text
    // colour is this table's `-1` row (`0x4c2435`).
    lua.globals().set(
        "GetItemQualityColor",
        lua.create_function(|lua, quality: i64| {
            // The reference reads its argument as a 32-bit int (`ftol`) and then compares it
            // unsigned, which is exactly this pair of casts.
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

    // `GetItemInfo(itemID | "item:id:enchant:randomProperty:suffix")` →
    //   itemName, itemLink, itemQuality, itemMinLevel, itemType, itemSubType,
    //   itemStackCount, itemEquipLoc, itemTexture
    //
    // **Nine values, and every one of them confirmed** against the registered binding
    // `0x48e070`, which ends `mov eax,0x9; ret`. The
    // signature is the whole point of the verb: a later client inserts `itemLevel`
    // at position 4 and pushes the required level to 5, and 36 corpus addons destructure this one
    // positionally — `Informant/Informant.lua:268` reads all nine and stores position 4 as
    // `['reqLevel']`, `Auctioneer/Database/AucItemDB.lua:288` as `useLevel`.
    //
    // Per value, with the instruction that pushes it:
    //
    //  1 `itemName`    `0x5d8b00(buf, 0x400, itemId, randomProperty)` — the template name with the
    //                  `ItemRandomProperties.dbc` suffix appended. **We push the bare template
    //                  name**: the suffix table is the same stated gap `ui_items::item_link`
    //                  already carries, one random-suffix arc, not per-call-site drift.
    //  2 `itemLink`    `0x48e1c9`: `SStrPrintf(buf, 0x400, "item:%d:%d:%d:%d", id, enchant,
    //                  randomProperty, suffix)` — the literal at `0x842d4c`. In 1.12 this is the
    //                  **item string**, NOT the coloured `|cff…|Hitem:…|h[Name]|h|r` hyperlink
    //                  (that is `GetContainerItemLink`'s shape and stays there). Auctioneer's
    //                  `getItemInfoFromBlizzard` names the return `itemString` and feeds it
    //                  straight back in, which only works because both ends are this shape.
    //  3 `itemQuality` `fild [record+0x1c]` — the same dword `0x495300` reads for the drag
    //                  payload.
    //  4 `itemMinLevel` `fild [record+0x3c]` — the **required** level. `ItemLevel` lives one dword
    //                  earlier at `+0x38` and is never pushed here. This is the trap.
    //  5 `itemType`    ItemClass.dbc's localized class name, or `""` (`0x48e236`).
    //  6 `itemSubType` ItemSubClass.dbc, VerboseName then DisplayName (`0x48e311`), or `""`.
    //  7 `itemStackCount` `fild [record+0x60]` — `Stackable`, not `MaxCount` (`+0x5c`).
    //  8 `itemEquipLoc` `[invType*4 + 0x83ddb0]` — the `INVTYPE_*` token; `""` at index 0.
    //  9 `itemTexture` `"%s%s%s"` over the icon dir, `"\\"` and the ItemDisplayInfo icon name.
    //
    // **A template the store has not seen yet returns nothing and records the ask**, exactly like
    // `BenillaGetItemStats` — and exactly like the reference, whose cache lookup `0x55ba30` fires
    // the query and returns null, taking the binding straight to `xor eax,eax; ret` (0 values).
    // The first call comes back empty and the answer arrives later; we never block or fabricate.
    lua.globals().set(
        "GetItemInfo",
        lua.create_function(|lua, arg: Value| {
            let (id, enchant, random_property, suffix) = parse_item_arg(&arg)?;
            let Ok(item_id) = u32::try_from(id) else {
                return Ok(MultiValue::new()); // negative / past u32: no record can hold it
            };
            let view = {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                let v = model.item_templates.get(&item_id).cloned();
                if v.is_none() && item_id != 0 {
                    model.item_stat_asks.insert(item_id);
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
                // The one value that can be genuinely absent rather than empty: a display row
                // with no icon column. `nil` rather than the reference's `Interface\Icons\`,
                // which is a path that cannot load — both read as "no icon" to `SetTexture`.
                match &v.icon {
                    Some(icon) => Value::String(lua.create_string(icon)?),
                    None => Value::Nil,
                },
            ]))
        })?,
    )?;

    // BenillaGetItemStats(itemId) → name, quality, invType, class, subclass, dmgMin, dmgMax,
    // dmgType, delayMs, armor, block, sellPrice — or nil (recording the ask).
    lua.globals().set(
        "BenillaGetItemStats",
        lua.create_function(|lua, item_id: u32| {
            let view = {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                let v = model.item_templates.get(&item_id).cloned();
                if v.is_none() && item_id != 0 {
                    model.item_stat_asks.insert(item_id);
                }
                v
            };
            let Some(v) = view else {
                return Ok(MultiValue::from_vec(vec![Value::Nil]));
            };
            let (dmg_min, dmg_max, dmg_type) = v.damages.first().copied().unwrap_or_default();
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&v.name)?),
                Value::Integer(i64::from(v.quality)),
                Value::Integer(i64::from(v.inventory_type)),
                Value::Integer(i64::from(v.class)),
                Value::Integer(i64::from(v.subclass)),
                Value::Number(f64::from(dmg_min)),
                Value::Number(f64::from(dmg_max)),
                Value::Integer(i64::from(dmg_type)),
                Value::Integer(i64::from(v.delay_ms)),
                Value::Integer(i64::from(v.armor)),
                Value::Integer(i64::from(v.block)),
                Value::Integer(i64::from(v.sell_price)),
            ]))
        })?,
    )
}

#[cfg(test)]
mod tests {
    use super::{item_usable, ItemTemplateView, PlayerReqState};
    use crate::script::UiScript;

    /// Every leg of the `0x5ea930` gate, one at a time against a passing baseline.
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

        // 1 · level: required 5 vs level 4 fails; exactly 5 passes (jg, not jge).
        let mut v = base_item.clone();
        v.required_level = 5;
        assert!(!item_usable(&v, &base_req, knows_none));
        let mut req = base_req.clone();
        req.level = 5;
        assert!(item_usable(&v, &req, knows_none));

        // 2/3 · class + race masks: the player's bit must be set; −1 has every bit.
        let mut v = base_item.clone();
        v.allowable_class = 1 << 3; // rogue-only (class 4)
        assert!(!item_usable(&v, &base_req, knows_none));
        let mut v = base_item.clone();
        v.allowable_race = 1 << 0; // human-only
        assert!(!item_usable(&v, &base_req, knows_none));

        // 4 · proficiency: a mask for the item class with the subclass bit clear fails — and
        // there is NO alternate walk here (a 2H axe reds even when the 1H bit is set).
        let mut v = base_item.clone();
        (v.class, v.subclass) = (2, 1); // Two-Handed Axe
        let mut req = base_req.clone();
        req.proficiency.insert(2, 1 << 0); // knows One-Handed Axes only
        assert!(!item_usable(&v, &req, knows_none));
        req.proficiency.insert(2, 1 << 1);
        assert!(item_usable(&v, &req, knows_none));
        // No mask for the class at all (consumables): the leg never fires.
        let mut v = base_item.clone();
        (v.class, v.subclass) = (0, 0);
        assert!(item_usable(&v, &base_req, knows_none));

        // 5 · skill: unknown skill fails even at rank 0; known compares value ≥ rank.
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

        // 6 · spell: required and not in the spellbook fails.
        let mut v = base_item.clone();
        v.required_spell = 9787; // Weaponsmith
        assert!(!item_usable(&v, &base_req, knows_none));
        assert!(item_usable(&v, &base_req, |id| id == 9787));

        // 7 · honor rank: the player's highest rank must reach it.
        let mut v = base_item.clone();
        v.required_honor_rank = 3;
        assert!(!item_usable(&v, &base_req, knows_none));
        let mut req = base_req.clone();
        req.honor_rank = 3;
        assert!(item_usable(&v, &req, knows_none));

        // 8 · city rank: no live data ever sets it, and the medals field it tests is never
        // written — a nonzero requirement can only fail.
        let mut v = base_item.clone();
        v.required_city_rank = 1;
        assert!(!item_usable(&v, &base_req, knows_none));

        // 9 · reputation: rank below the requirement fails; at it, passes; an unknown faction
        // is rank 0 (fails any nonzero requirement, passes a zero one).
        let mut v = base_item.clone();
        (v.required_rep_faction, v.required_rep_rank) = (87, 5);
        assert!(!item_usable(&v, &base_req, knows_none));
        let mut req = base_req.clone();
        req.rep_ranks.insert(87, 5);
        assert!(item_usable(&v, &req, knows_none));
        let mut v = base_item.clone();
        (v.required_rep_faction, v.required_rep_rank) = (87, 0);
        assert!(item_usable(&v, &base_req, knows_none));

        // An unpushed player state (level 0 — unreachable in the real client) declines to judge.
        let mut v = base_item.clone();
        v.required_level = 60;
        assert!(item_usable(&v, &PlayerReqState::default(), knows_none));
    }

    /// The ask-once loop end-to-end: a miss answers nil AND records the ask; the app's push makes
    /// the next read answer the stat head; the push clears the pending ask.
    #[test]
    fn miss_records_ask_and_push_serves_the_stats() {
        let mut s = UiScript::new().unwrap();
        assert!(s
            .eval::<bool>("return BenillaGetItemStats(25) == nil")
            .unwrap());
        assert_eq!(s.take_item_stat_asks(), vec![25]);

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
        let (name, quality, inv): (String, i64, i64) =
            s.eval("return BenillaGetItemStats(25)").unwrap();
        assert_eq!((name.as_str(), quality, inv), ("Worn Shortsword", 1, 21));
        assert!(s.take_item_stat_asks().is_empty(), "push cleared the ask");
        // id 0 (an unresolved row) never records a junk ask.
        assert!(s
            .eval::<bool>("return BenillaGetItemStats(0) == nil")
            .unwrap());
        assert!(s.take_item_stat_asks().is_empty());
    }
}

#[cfg(test)]
mod get_item_info_tests {
    use super::{equip_loc_token, reference_atoi, ItemTemplateView};
    use crate::script::UiScript;

    /// **Worn Shortsword (25)**, with every field taken from the live vmangos `item_template`
    /// row so the assertions below are about real data:
    /// `class 2, subclass 7, quality 1, inventory_type 21, required_level 1, item_level 2,
    /// max_count 0, stackable 1`. The two app-resolved names are what the shipped
    /// `ItemClass.dbc` / `ItemSubClass.dbc` rows hold for `(2, 7)`.
    fn worn_shortsword() -> ItemTemplateView {
        ItemTemplateView {
            name: "Worn Shortsword".into(),
            quality: 1,
            class: 2,
            subclass: 7,
            inventory_type: 21,
            item_type: Some("Weapon".into()),
            // VerboseName, not the tooltip cell's "Sword" — the binding's `0x48e311` two-step.
            item_sub_type: Some("One-Handed Swords".into()),
            // The pair that makes position 4 falsifiable: this item's REQUIRED level is 1 and its
            // ITEM level is 2. Ashbringer (13262) is the loud version — required 60, item 76.
            required_level: 1,
            stackable: 1,
            max_count: 0,
            icon: Some("Interface\\Icons\\INV_Sword_04".into()),
            ..Default::default()
        }
    }

    /// The whole signature, destructured in order exactly as `Informant/Informant.lua:268` and
    /// `Auctioneer/Database/AucItemDB.lua:288` destructure it.
    ///
    /// **The arity assertion is half the test.** A regression to a later client's shape inserts
    /// `itemLevel` at position 4 and makes this ten values; the COUNT is the only check that
    /// notices, because every individual read still "works".
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
        // Return 2 is the ITEM STRING, not the coloured hyperlink (`"item:%d:%d:%d:%d"` at
        // 0x842d4c). A number argument leaves the other three fields at 0.
        assert_eq!(link, "item:25:0:0:0");
        assert_eq!(quality, 1);
        assert_eq!(min_level, 1);
        assert_eq!(ty, "Weapon");
        assert_eq!(sub_ty, "One-Handed Swords");
        assert_eq!(stack, 1);
        assert_eq!(equip, "INVTYPE_WEAPONMAINHAND");
        assert_eq!(texture.as_deref(), Some("Interface\\Icons\\INV_Sword_04"));
    }

    /// **Position 4 is the REQUIRED level, and position 5 is a string.** This is the whole reason
    /// decision 1199 exists, one verb later: every client after 1.12 inserts `itemLevel` at 4 and
    /// pushes `itemMinLevel` to 5, so a regression makes position 5 a *number*. Both halves are
    /// asserted, against a template whose two levels genuinely differ (Ashbringer: required 60,
    /// item level 76 — verified in the live vmangos `item_template`).
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
                required_level: 60, // item_level is 76 and is NEVER returned by 1.12
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

    /// **Position 7 is `Stackable`, not `MaxCount`** — `[record+0x60]`, the dword *after*
    /// `MaxCount` at `+0x5c`. The view carries both and they are not interchangeable: Linen Cloth
    /// is `max_count 0, stackable 20` in the live `item_template`, so reading the wrong one gives
    /// every reagent counter in the corpus a stack size of zero.
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
        // InventoryType 0 answers the EMPTY STRING, not nil — index 0 of the 0x83ddb0 table
        // points at the shared "" at 0x882748.
        assert_eq!(equip, "");
    }

    /// The ask-once loop, the reference's own uncached-item behaviour: the cache lookup
    /// (`0x55ba30`) fires the query and returns null, and the binding falls to
    /// `xor eax,eax; ret` — **no values at all**, not a nil.
    #[test]
    fn an_uncached_id_returns_nothing_and_records_the_ask() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(
            s.arity("GetItemInfo(2589)").unwrap(),
            0,
            "an unseen template returns NO values"
        );
        assert!(s.eval::<bool>("return (GetItemInfo(2589)) == nil").unwrap());
        assert_eq!(s.take_item_stat_asks(), vec![2589], "the ask was recorded");

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

        // Id 0 and a negative id are not askable — no junk query goes out for them.
        assert_eq!(s.arity("GetItemInfo(0)").unwrap(), 0);
        assert_eq!(s.arity("GetItemInfo(-5)").unwrap(), 0);
        assert!(s.take_item_stat_asks().is_empty());
    }

    /// **The argument forms, which are narrower than the documentation** (see `parse_item_arg`):
    /// a number, a string Lua coerces to one, or an `item:`-prefixed *item string*. A bare item
    /// NAME and a full `|cff…|Hitem:…` hyperlink both resolve to id 0 on the real client, because
    /// `strnicmp(s, "item:", 5)` is the only string test there is.
    #[test]
    fn the_argument_forms_are_the_references_own() {
        let mut s = UiScript::new().unwrap();
        s.set_item_template(25, worn_shortsword());

        // A number, and a string Lua's own coercion reads as one (`lua_isnumber`, 0x6f34d0) —
        // `GetItemInfo(tostring(id))` is a real corpus call shape.
        for arg in ["25", "\"25\"", "25.7"] {
            assert_eq!(
                s.eval::<String>(&format!("return (GetItemInfo({arg}))"))
                    .unwrap(),
                "Worn Shortsword",
                "argument {arg}"
            );
        }

        // The item string — the form Auctioneer builds (`AucItemDB.lua:333`). Its enchant /
        // random-property / suffix fields are echoed back into return 2 verbatim, which is what
        // makes Auctioneer's feed-the-answer-back-in round trip.
        let (name, link): (String, String) = s
            .eval("local a,b = GetItemInfo(\"item:25:2564:7:0\") return a,b")
            .unwrap();
        assert_eq!(name, "Worn Shortsword");
        assert_eq!(link, "item:25:2564:7:0");
        // Case-insensitive prefix, and a truncated string still parses (atoi stops at the NUL).
        assert_eq!(
            s.eval::<String>("local _, link = GetItemInfo(\"ITEM:25\") return link")
                .unwrap(),
            "item:25:0:0:0"
        );

        // A bare NAME and a full hyperlink: no values. Neither begins with "item:", so the id
        // stays 0 and the record lookup fails — the real client answers nothing here too, and
        // neither records an ask.
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

        // Neither a number nor a string raises the binding's own usage error (`0x842d24`, pushed
        // through the `luaL_error` shape at `0x6f4940`).
        let err = s
            .eval::<mlua::Value>("return GetItemInfo(nil)")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("Usage: GetItemInfo(itemID|\"itemlink\")"),
            "expected the reference's usage error, got {err}"
        );
    }

    /// The `INVTYPE_*` table, transcribed from the 30-pointer array at `0x83ddb0`: both ends,
    /// the four tokens the shipped `GlobalStrings.lua` never defines, and the empty-string slots.
    #[test]
    fn the_equip_loc_tokens_are_the_binarys_table() {
        assert_eq!(equip_loc_token(0), "", "index 0 is the shared \"\"");
        assert_eq!(equip_loc_token(1), "INVTYPE_HEAD");
        assert_eq!(equip_loc_token(3), "INVTYPE_SHOULDER", "singular");
        assert_eq!(equip_loc_token(9), "INVTYPE_WRIST", "singular");
        assert_eq!(equip_loc_token(10), "INVTYPE_HAND", "singular");
        assert_eq!(equip_loc_token(17), "INVTYPE_2HWEAPON");
        assert_eq!(equip_loc_token(20), "INVTYPE_ROBE");
        // The four with no GlobalString to localize them — the reference's own asymmetry.
        assert_eq!(equip_loc_token(24), "INVTYPE_AMMO");
        assert_eq!(equip_loc_token(25), "INVTYPE_THROWN");
        assert_eq!(equip_loc_token(26), "INVTYPE_RANGEDRIGHT");
        assert_eq!(equip_loc_token(27), "INVTYPE_QUIVER");
        assert_eq!(equip_loc_token(28), "INVTYPE_RELIC", "the last real token");
        assert_eq!(equip_loc_token(29), "", "the array's trailing \"\"");
        assert_eq!(equip_loc_token(9999), "", "past the array");
    }

    /// The reference's `atoi` (`0x64ac60`): no whitespace skip, no `+`, stop at the first
    /// non-digit. The last two cases are why this is not `str::parse`.
    #[test]
    fn the_field_parser_is_the_references_atoi() {
        assert_eq!(reference_atoi("2589"), 2589);
        assert_eq!(reference_atoi("-5"), -5);
        assert_eq!(reference_atoi(""), 0);
        assert_eq!(reference_atoi("abc"), 0);
        assert_eq!(reference_atoi(" 12"), 0, "no leading-whitespace skip");
        assert_eq!(reference_atoi("+12"), 0, "no unary plus");
        assert_eq!(reference_atoi("12abc"), 12, "stops at the first non-digit");
    }
}

#[cfg(test)]
mod quality_color_tests {
    use crate::script::UiScript;

    /// The seven colours, confirmed against the reference's own table (`0x5291d0`) — and the
    /// two edges, which are where every reimplementation of this goes wrong.
    ///
    /// The escape string matters as much as the floats: the reference returns it as the fourth
    /// value and addons splice it straight into a link (`ITEM_QUALITY_COLORS[q].hex .. name`), so
    /// a missing or differently-cased one shows as literal text in a chat line.
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

        // The floats are `byte / 255`, not a rounded approximation — Epic's red is 0xa3/255.
        let r = s
            .eval::<f64>("local r = GetItemQualityColor(4) return r")
            .unwrap();
        assert!((r - 163.0 / 255.0).abs() < 1e-9, "epic red was {r}");

        // `>= 7` clamps to COMMON (index 1), the accessor `0x52ad70`'s own rule — not to Artifact,
        // and never to nil, because the caller concatenates the result.
        assert_eq!(hex(7), "|cffffffff");
        assert_eq!(hex(99), "|cffffffff");
        // …and the compare is UNSIGNED, so every negative takes the SAME branch and reads Common.
        // This is the row `UIParent.lua`'s `for i = -1, 6` builds, and the colour a loot row wears
        // while its item template is uncached (`GetLootSlotInfo` answers -1 there). It read Poor
        // until 1805; there is no out-of-bounds read to reproduce, the `jb` catches it first.
        assert_eq!(hex(-1), "|cffffffff");
        assert_eq!(hex(-99), "|cffffffff");
    }
}
