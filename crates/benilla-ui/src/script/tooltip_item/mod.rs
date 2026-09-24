//! The engine **item-tooltip renderer** (decision 0274 P1) — the one line law every item hover
//! renders through, mirroring the real client's single shared C++ renderer (`0x52b650`, behind
//! 8 of the 9 `Set*Item` bindings). The entry methods
//! (`SetBagItem`, `SetMerchantItem`, `SetBuybackItem`, and the prefixed `BenillaSetItemById` —
//! ours, because 1.12 has no id-keyed door onto the shared renderer) register into the
//! GameTooltip kind table beside the widget verbs ([`super::tooltip`]).
//!
//! **The line law is BYTE-VERIFIED** — the 0274 §5 verdict on `0x52b650`'s emission order:
//! every family's order, gate, and color pointer is the binary's, and every sentence is a KEY
//! resolved off the player's own `GlobalStrings.lua` at render time. Not yet
//! built (feeds pending, laws recorded): the instance families (soulbound override, enchants,
//! made-by, live durability, cooldown-remaining).
//! Residual INTERIMs cited inline: the dual-wield/off-hand proficiency exception
//! (`0x5eab70`), the type cell's override red, the set-owned count source.
//!
//! **Compare mode** (0274 P4, re-based by 2202, scoped back to the reference by 2210, and corrected
//! to match the reference by 2216): `SetInventoryItem` on an ARMED shopping tooltip renders the
//! equipped item's ORDINARY tooltip plus ONE extra line, first — the gray "Currently Equipped"
//! (`[arg+0x18]≠0`). That is the entire difference. The builder's other flag, p4 `[arg+0x14]`, *is*
//! a compact mode (white name, stat body jumped, early-return at `0x52e14c`) — but the two compare
//! call sites pass it **zero** (`0x5362c0`, `0x53602a`), so a shopping plate shows a
//! quality-colored name and the full body, and a client that abbreviates here is wrong. **Nothing
//! in this engine seats a shopping plate**: the two callers that arm this mode are
//! [`compare_against_worn`]'s own bindings — `SetMerchantCompareItem` and `SetAuctionCompareItem` —
//! and the FrameXML that calls them (`MerchantFrame.xml:63-80`, `AuctionFrame`) owns the plates'
//! geometry and lifetime, exactly as in 1.12.1. There is no hover compare in the reference at all:
//! `SHOW_COMPARE_TOOLTIP` has zero fire sites in the whole image (its only two dynamic fire sites,
//! `0x49211a`/`0x49630d`, can never produce event 377), so `PaperDollFrame.lua`'s slot listener is
//! dead code there.
//!
//! **`nameOnly`** (p4 `[arg+0x14]`, the flag 2216 had tangled with the header; built by 2224): the
//! builder's compact mode, and a census of the reference's call sites found exactly one door onto
//! it — this module's `SetInventoryItem`, third argument, a number `> 0`. It is what the image's
//! own usage string `0x8552dc` calls it. No stock FrameXML caller passes it (all 8 sites are
//! two-argument), so it is addon surface; it is nonetheless live code, and the mode is **trimmed,
//! not bare** — the bind/lock region and the stat body are jumped and the tail is cut, but the
//! slot/type cell, durability, every requirement line and the spell triggers all still print. Both
//! flags ride in [`render::BuilderFlags`], a struct precisely so they cannot be collapsed again.
//!
//! **The red "you can't use this" law** (the director's explicit ask): the AddLine colour pointer
//! `0xc0d390` is `0xffff2020`, applied to requirement lines the ACTIVE player fails (level,
//! class/race lists, skill rank, required spell), to LOCKED, to broken durability, and
//! unconditionally to "Already known"; the NAME never recolors. Compared against
//! [`super::PlayerReqState`] + the spellbook.
//!
//! The sell-price money row is the byte-verified law: only a REAL-INSTANCE source (`SetBagItem`)
//! while the merchant window is open and repair mode is off — the engine computes
//! `SellPrice × stack` and fires the `OnTooltipAddMoney` script; FrameXML renders the coins
//! (`SetTooltipMoney`). A zero sell price in that context prints `ITEM_UNSELLABLE`
//! (`0x854a74`, pushed at `0x52e4a3`). Template sources (`SetMerchantItem`/`BenillaSetItemById`/quest
//! rows) never show money, per the same law.

use mlua::{Lua, MultiValue, Table, Value};

use super::object::frame_handle_of;
use super::tooltip::{append_line, clear_content, fire_cleared, show_or_hide_empty};
use super::{ItemTemplateView, Model};

mod names;
mod render;

use names::{quality_color, GRAY, WHITE};
use render::{render_view, BuilderFlags};

/// The compare tooltips' shared body — what `SetMerchantCompareItem 0x536080` and
/// `SetAuctionCompareItem 0x535d70` both run once their wrapper has an offered template and a
/// re-based `offset` (`0x5d9f30` gates the class test): walk the offered type's candidate slots
/// (`0x809200[InventoryType]`), skipping EMPTY slots without decrementing and slots whose
/// worn item's CLASS differs, until `offset` occupied matches have been passed; then fill the
/// ordinary EQUIPPED-item tooltip with the compare header on — the FULL body (`p4 = 0`) plus one
/// gray `CURRENTLY_EQUIPPED` line (`p5 = 1`), by arming the `equipped_header_armed` latch and running
/// `SetInventoryItem`, which is what the reference does too:
/// `0x536080` calls the ordinary item builder `0x52b650` with the header flag set. The NUMBER 1 on
/// success; nil on a miss, which does NOT clear the tooltip (the reference's nil exit touches
/// nothing else).
fn compare_against_worn(
    lua: &Lua,
    this: &Table,
    h: crate::widget::FrameHandle,
    offered: &ItemTemplateView,
    mut left: f64,
) -> mlua::Result<Value> {
    let found = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        let mut hit = None;
        for &slot in equip_slots_for(offered.inventory_type) {
            let Some(worn) = model
                .inv_slot("player", slot as usize)
                .filter(|s| s.item_id != 0)
            else {
                continue; // empty: skipped WITHOUT decrementing
            };
            let same_class = model
                .item_templates
                .get(&worn.item_id)
                .is_some_and(|w| w.class == offered.class);
            if !same_class {
                continue;
            }
            if left <= 0.0 {
                hit = Some(slot as usize);
                break;
            }
            left -= 1.0;
        }
        hit
    };
    let Some(slot) = found else {
        return Ok(Value::Nil);
    };
    {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        if let Ok(t) = super::tooltip::tip_mut(&mut model, h) {
            t.equipped_header_armed = true;
        }
    }
    this.get::<mlua::Function>("SetInventoryItem")?
        .call::<mlua::MultiValue>((this.clone(), "player", slot))?;
    Ok(Value::Integer(1))
}

/// Look up the store; a miss records the ask (the app sends `CMSG_ITEM_QUERY` and pushes back —
/// the hover's re-enter loop repaints on arrival).
fn view_of(lua: &Lua, item_id: u32) -> Option<ItemTemplateView> {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let v = model.item_templates.get(&item_id).cloned();
    if v.is_none() && item_id != 0 {
        model.item_stat_asks.insert(item_id);
    }
    v
}

/// The enchant lines a **random-suffix roll** contributes, out of the pushed roll table — the
/// reference's `0x52b7bf` copy of the suffix row's five ids into enchant slots 2..6, which every
/// block-supplying source with no item object relies on (loot, links, auction rows, the roll
/// window). `0`, or an id the table doesn't name, contributes nothing.
fn roll_enchants(lua: &Lua, random_property_id: u32) -> Vec<crate::script::EnchantView> {
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    model
        .random_properties
        .get(&random_property_id)
        .map(|v| v.enchants.clone())
        .unwrap_or_default()
}

/// The bracketed name out of an `|Hitem:…|h[Name]|h` link (the container slot's own display
/// name — the miss-path fallback line's source).
fn link_name(link: &str) -> Option<&str> {
    let start = link.find('[')? + 1;
    let end = link[start..].find(']')? + start;
    Some(&link[start..end])
}

/// Fire the engine→Lua money hand-off (`OnTooltipAddMoney(copper)` — the byte-verified protocol:
/// the engine computes, FrameXML's `SetTooltipMoney` renders).
fn fire_add_money(lua: &Lua, h: crate::widget::FrameHandle, copper: u64) {
    let id = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        model.frame_id(h)
    };
    if let Err(e) = super::event::fire_widget_handler(
        lua,
        id,
        "OnTooltipAddMoney",
        vec![Value::Number(copper as f64)],
    ) {
        lua.app_data_mut::<Model>()
            .expect("model app_data")
            .record_script_error(e.to_string());
    }
}

/// The `|Hitem:` payload's numeric fields — the full escaped shape
/// (`|cff…|Hitem:2947:0:584:0|h[Name]|h|r`) or a bare `item:2947`.
///
/// The four are `(item id, enchant id, randomPropertyId, uniqueId)` — the reference's own link
/// format `"%s|Hitem:%d:%d:%d:%d|h[%s]|h%s"` (`0x8549c8`), and `SetHyperlink 0x532181` parses them
/// straight into the tooltip's instance block: token 1 → enchant slot 0 (`+0x3d0`), **token 2 →
/// `+0x424`, the roll** (which `0x52b7bf` then expands into slots 2..6), token 3 → `+0x420`, which
/// nothing reads. Missing fields read `0`, like the bare `item:id` shape an addon may pass.
/// The `enchant:<spellId>` of an enchant hyperlink, or `None` for any other link.
fn hyperlink_enchant_spell(link: &str) -> Option<u32> {
    let at = link.find("enchant:")? + 8;
    let tail = &link[at..];
    let end = tail
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(tail.len());
    tail[..end].parse().ok().filter(|&id: &u32| id != 0)
}

fn hyperlink_item_fields(link: &str) -> Option<(u32, u32, u32)> {
    let at = link.find("item:")? + 5;
    let tail = &link[at..];
    let end = tail.find("|h").unwrap_or(tail.len());
    let mut fields = tail[..end]
        .split(':')
        .map(|f| f.trim().parse().unwrap_or(0));
    let item_id: u32 = fields.next()?;
    let enchant_id = fields.next().unwrap_or(0);
    let random_property_id = fields.next().unwrap_or(0);
    (item_id != 0).then_some((item_id, enchant_id, random_property_id))
}

/// The shared id-keyed render (`BenillaSetItemById`/`SetHyperlink`): template hit → the full line law
/// (+ the compare arm when this is the main GameTooltip); miss → the ask + a name-only line.
fn render_by_id(
    lua: &Lua,
    this: &Table,
    item_id: u32,
    fb_name: Option<String>,
    fb_q: Option<u32>,
) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        clear_content(&mut model, h);
    }
    fire_cleared(lua, h);
    match view_of(lua, item_id) {
        Some(v) => {
            render_view(lua, this, &v, BuilderFlags::default(), None)?;
        }
        None => {
            if let Some(name) = fb_name {
                append_line(
                    lua,
                    this,
                    (name, quality_color(fb_q.unwrap_or(1))),
                    None,
                    false,
                )?;
            }
        }
    }
    show_or_hide_empty(lua, h);
    Ok(())
}

/// The quest-item hover: the row's item by id, or an empty tooltip when the row is not there.
fn set_quest_item_view(
    lua: &Lua,
    this: &Table,
    item: Option<crate::script::quest::QuestItemView>,
) -> mlua::Result<()> {
    match item {
        Some(it) => render_by_id(lua, this, it.item_id, it.name.clone(), Some(it.quality)),
        None => {
            let h = frame_handle_of(lua, this)?;
            {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                clear_content(&mut model, h);
            }
            super::tooltip::show_or_hide_empty(lua, h);
            Ok(())
        }
    }
}

/// The paperdoll slots an item of this InventoryType equips into (the 1.12
/// `GetInventorySlotInfo` slot ids) — the compare drive's candidate slots. Two-slot families
/// (rings, trinkets, one-hand weapons, and a two-hander displacing both hands) can fill both
/// shopping plates. Twin of the app-side `ui_items::find_equip_slot` (the equip-click fit rule) —
/// one law, two consumers.
fn equip_slots_for(inventory_type: u32) -> &'static [u32] {
    match inventory_type {
        1 => &[1],             // head
        2 => &[2],             // neck
        3 => &[3],             // shoulder
        4 => &[4],             // shirt
        5 | 20 => &[5],        // chest / robe
        6 => &[6],             // waist
        7 => &[7],             // legs
        8 => &[8],             // feet
        9 => &[9],             // wrist
        10 => &[10],           // hands
        11 => &[11, 12],       // finger
        12 => &[13, 14],       // trinket
        16 => &[15],           // back
        19 => &[19],           // tabard
        13 => &[16, 17],       // one-hand
        21 => &[16],           // main hand
        14 | 22 | 23 => &[17], // shield / off-hand weapon / held
        // Two-hand: **main hand ONLY**, which is not the obvious answer. A 2H displaces both
        // hands when equipped, and this table was written `[16, 17]` on that reasoning. The
        // reference's own mask disagrees: `0x809200[17]` is `0x8000`, bit 15, main hand alone.
        // It is a COMPARE-CANDIDATE table, not an
        // occupancy one — the question it answers is "which worn item is this offering to
        // replace", and for a two-hander that is the weapon in your main hand.
        17 => &[16],
        15 | 25 | 26 | 28 => &[18], // bow / thrown / wand-gun / relic
        _ => &[],
    }
}

/// Register the item content channels into the GameTooltip kind method table.
pub(super) fn install_methods(lua: &Lua, m: &Table) -> mlua::Result<()> {
    // GameTooltip:BenillaSetItemById(itemId [, fallbackName, fallbackQuality]) — the id-keyed
    // hover (quest reward rows, loot rows).
    //
    // **Ours, and the `Benilla` prefix is what keeps it honest.** 1.12's nine `Set*Item` bindings
    // all name a CONTAINER — a bag slot, a merchant row, a loot slot — because the reference's
    // callers always have one; nothing there takes a bare item id. Ours do: the channels in this
    // module reach [`render_by_id`] as a Rust call, but the ones that live in a sibling module
    // (`tooltip_spell`'s action-bar item arm, `SetTrainerService`, `SetCraftSpell`) reach it
    // through the wrapper table, and that needs a NAME. Spelling that name like a WoW function
    // would be an unexplained superset an addon can feature-detect (1188; the census that moved
    // it, 2142); under the prefix it is unreachable by accident from an addon that means to call
    // a WoW function, and no addon has reason to call it at all.
    //
    // Template source: no money row, per the byte-verified sell-price law.
    m.set(
        "BenillaSetItemById",
        lua.create_function(
            |lua, (this, item_id, fb_name, fb_q): (Table, u32, Option<String>, Option<u32>)| {
                render_by_id(lua, &this, item_id, fb_name, fb_q)
            },
        )?,
    )?;

    // GameTooltip:SetQuestItem(type, index) / SetQuestLogItem(type, index) — the quest-giver
    // panels' and the quest log's reward/choice/required hovers (stock QuestFrameTemplates.xml:148,
    // QuestLogFrame.xml:113; bindings 0x533610 / 0x533760, 3 args with self, 0 returns). `type` is
    // the same "choice" | "reward" | "required" key `GetQuestItemInfo` takes, `index` the 1-based
    // slot; the item is rendered by id through the shared renderer, with the row's own name and
    // quality as the fallbacks a template that has not arrived yet paints from. An unknown type or
    // an out-of-range index shows nothing — the reference's tooltip stays empty too.
    m.set(
        "SetQuestItem",
        lua.create_function(|lua, (this, kind, index): (Table, String, usize)| {
            let item = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model.quest.as_ref().and_then(|q| {
                    crate::script::quest::item_vec(q, &kind)
                        .and_then(|v| index.checked_sub(1).and_then(|n| v.get(n)))
                        .cloned()
                })
            };
            set_quest_item_view(lua, &this, item)
        })?,
    )?;
    m.set(
        "SetQuestLogItem",
        lua.create_function(|lua, (this, kind, index): (Table, String, usize)| {
            let item = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model.selected_quest_detail().and_then(|d| {
                    let v = match kind.as_str() {
                        "choice" => Some(&d.choices),
                        "reward" => Some(&d.rewards),
                        _ => None,
                    };
                    v.and_then(|v| index.checked_sub(1).and_then(|n| v.get(n)))
                        .cloned()
                })
            };
            set_quest_item_view(lua, &this, item)
        })?,
    )?;

    // GameTooltip:SetHyperlink(link) — the chat-link tooltip (ref ItemRef.lua's SetItemRef →
    // ItemRefTooltip:SetHyperlink). Accepts the full escaped link or a bare "item:<id>";
    // non-item links (player/spell/quest) have no tooltip surface yet and no-op.
    m.set(
        "SetHyperlink",
        lua.create_function(|lua, (this, link): (Table, String)| {
            // The client's only other link kind: `|Henchant:<spellId>|h[name]|h`, the craft
            // window's enchant link (`GetCraftItemLink`, 1973), which `SetHyperlink` consumes
            // behind the same castUI gate its producer stands behind (`0x532243`) — the spell's
            // own tooltip, by id.
            if let Some(spell) = hyperlink_enchant_spell(&link) {
                return super::tooltip_spell::set_spell_by_id(
                    lua,
                    &this,
                    spell,
                    link_name(&link).map(str::to_string),
                    Default::default(),
                    None,
                );
            }
            let Some((id, _enchant_id, roll)) = hyperlink_item_fields(&link) else {
                return Ok(());
            };
            let h = frame_handle_of(lua, &this)?;
            {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                clear_content(&mut model, h);
            }
            fire_cleared(lua, h);
            // A link is a **block source** (`SetHyperlink 0x532181` passes p6=1), so it never
            // reaches the `<Random enchantment>` arm (`0x52cc33`) and it does show the roll's own
            // lines — decision 0920's prose had that half backwards. The NAME comes off the link's
            // own brackets: the sender's client built that text with the suffix already joined
            // (`0x5d8b00` feeds the link builder), so re-deriving it would only invite the two to
            // disagree.
            //
            // The link's ENCHANT field (slot 0, a permanent enchant like "Crusader") is parsed but
            // not yet rendered: naming it needs the `SpellItemEnchantment` row, and no benilla
            // surface writes that field into a link it composes today. A stated gap, not drift.
            let inst = render::ItemInstance {
                name: link_name(&link).map(str::to_string),
                enchants: roll_enchants(lua, roll),
                ..Default::default()
            };
            match view_of(lua, id) {
                Some(v) => {
                    render_view(lua, &this, &v, BuilderFlags::default(), Some(&inst))?;
                    // Unchanged from the `render_by_id` this leg used to share: a link dropped on
                }
                None => {
                    if let Some(name) = link_name(&link) {
                        append_line(
                            lua,
                            &this,
                            (name.to_string(), quality_color(1)),
                            None,
                            false,
                        )?;
                    }
                }
            }
            show_or_hide_empty(lua, h);
            Ok(())
        })?,
    )?;

    // GameTooltip:SetInventoryItem(unit, slot) → hasItem, hasCooldown, repairCost — the equipped-slot hover
    // (paperdoll slots, buff-frame weapon enchants) and the shopping-compare listener's render
    // (ref PaperDollFrame.lua:626). **Unit-keyed** through `Model::inv_slot`, the same router the
    // `GetInventoryItem*` getters use: `"player"` from the self feed, the inspected token from the
    // PUBLIC visible-item view (the ref's inspect slot OnEnter calls exactly this,
    // `InspectPaperDollFrame.xml:20`). An inspected item carries no durability/creator, so those
    // lines simply don't render — the reference's own inspect tooltip shape. On an ARMED shopping
    // tooltip this renders the byte law's compare shape; the arm is consumed either way.
    // hasCooldown is nil INTERIM (no equipped-cooldown feed yet — its truthiness gates the ref's
    // re-poll only).
    //
    // **THREE returns, not two.** The reference's own `PaperDollFrame.lua:741` reads
    // `local hasItem, hasCooldown, repairCost = GameTooltip:SetInventoryItem("player", this:GetID())`
    // — Blizzard's file, out of the shipped `patch.MPQ`, and its very next use is
    // `if ( InRepairMode() and repairCost and (repairCost > 0) ) then ... SetTooltipMoney(...)`.
    // (l.632 in the same file reads only two, which is a caller not needing the third, not a
    // different contract.) We pushed two, so pfUI's durability scan died on
    // `panel.lua:499: attempt to perform arithmetic on local 'repCost' (a nil value)`.
    //
    // `repairCost` is **0 INTERIM**, the same posture and the same reason as `SetBagItem`'s below:
    // the per-item repair feed is the paper-doll arc's and does not exist yet. Zero rather than nil
    // because the reference **always pushes a number** there — `0x534975` reads `fild [ebp-0x30];
    // call 0x6f3810` on `SetBagItem`'s unconditional leg — and an undamaged item's cost genuinely
    // is 0, so the interim value is a *real* value of the domain rather than a stand-in. Every
    // reference consumer guards on `> 0`.
    //
    // Arity provenance, stated because it differs from the rest of this module: the *three* is the
    // reference's own consumer, not a byte-read `mov eax, 3`. `SetBagItem`'s two IS byte-verified
    // (`mov eax,2` @0x534985). Reading `0x532ee0`'s tail from the bytes would upgrade this line.
    m.set(
        "SetInventoryItem",
        lua.create_function(
            |lua, (this, unit, slot, name_only): (Table, String, usize, Value)| {
                let h = frame_handle_of(lua, &this)?;
                // **The third argument the binary names itself**: `0x8552dc` carries
                // `"Usage: SetInventoryItem(unit, slot [, nameOnly])"`, and `0x533027`–`0x53304c` is
                // its gate — the flag seeds 0 and becomes 1 only on `lua_isnumber(L,4)` AND
                // `lua_tonumber(L,4) > 0.0` STRICTLY. A miss does not raise; it leaves the zero and
                // builds the ordinary tooltip. Three of this binding's four builder legs pass it
                // through (the fourth, `0x533106`, does not).
                let name_only = super::binding_abi::positive_number_flag(lua, name_only)?;
                let (item_id, name, quality, inst, currently_equipped) = {
                    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                    let armed = match super::tooltip::tip_mut(&mut model, h) {
                        Ok(t) => std::mem::take(&mut t.equipped_header_armed),
                        Err(_) => false,
                    };
                    let view = model
                        .inv_slot(&unit, slot)
                        .filter(|s| s.item_id != 0)
                        .map(|s| {
                            (
                                s.item_id,
                                s.name.clone(),
                                s.quality,
                                render::ItemInstance {
                                    // The slot's own name — app-composed, so it carries the
                                    // random-suffix roll off `ITEM_FIELD_RANDOM_PROPERTIES_ID`.
                                    name: s.name.clone(),
                                    durability: s.durability,
                                    creator: s.creator.clone(),
                                    has_text: false,
                                    flags: s.flags,
                                    already_bound: s.already_bound,
                                    // No petition line on the doll: a charter has `InventoryType = 0`
                                    // and cannot be equipped, so this hover can never be over one.
                                    petition: None,
                                    enchants: s.enchants.clone(),
                                    // `SetInventoryItem 0x532ee0` also has p6=0 legs (`0x533106`,
                                    // `0x5332ad`) — the "this binding can never emit OPENABLE"
                                    // claim is dead here too. Which leg each takes is not pinned,
                                    // and the case is unobservable anyway:
                                    // nothing openable is equippable, so the doll hover has no clam to
                                    // show. Left `false` deliberately — inventing a selector we have
                                    // not read would be the §4 trade, and there is nothing to gain.
                                    openable_source: false,
                                    duration_ms: s.duration_ms,
                                },
                            )
                        });
                    match view {
                        Some((id, name, q, inst)) => (id, name, q, inst, armed),
                        // An EMPTY slot answers `nil` for hasItem — and still pushes the other two,
                        // because the reference's own caller destructures all three unconditionally
                        // and only then tests `hasItem`. Answering one value here would hand
                        // `PaperDollItemSlotButton_OnEnter` a nil `repairCost` on every empty slot,
                        // which its `repairCost and (repairCost > 0)` guard survives but pfUI's bare
                        // `totalRep + repCost` does not.
                        None => {
                            return Ok(MultiValue::from_vec(vec![
                                Value::Nil,
                                Value::Nil,
                                Value::Integer(0),
                            ]))
                        }
                    }
                };
                // p5 and p4, carried as one value but never as one flag (2216).
                let flags = BuilderFlags {
                    currently_equipped,
                    name_only,
                };
                {
                    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                    clear_content(&mut model, h);
                }
                fire_cleared(lua, h);
                match view_of(lua, item_id) {
                    Some(v) => render_view(lua, &this, &v, flags, Some(&inst))?,
                    None => {
                        // Template in flight — the slot view's own name holds the plate (the same
                        // 0138 posture as SetBagItem's miss path).
                        // The compare header is a key like every other sentence (2045); this
                        // fallback path shows it for exactly the reason the full render does.
                        if flags.currently_equipped {
                            if let Some(t) = crate::strings::global(lua, "CURRENTLY_EQUIPPED") {
                                append_line(lua, &this, (t, GRAY), None, false)?;
                            }
                        }
                        if let Some(name) = name {
                            let color = if flags.name_only {
                                WHITE
                            } else {
                                quality_color(quality.max(0) as u32)
                            };
                            append_line(lua, &this, (name, color), None, false)?;
                        }
                    }
                }
                show_or_hide_empty(lua, h);
                Ok(MultiValue::from_vec(vec![
                    Value::Integer(1),
                    // `hasCooldown` — **broader than its name**, and this is the one arm of it we can
                    // answer. The builder's Lua return is `[ebp-0x38]`, set by FOUR sites, only one of
                    // which is the cooldown line: LOCKED_WITH_ITEM, the temporary-enchant countdown,
                    // **the item's own duration line** (`0x52ce0d`), and ITEM_COOLDOWN_TIME. So a
                    // duration-bearing item answers truthy here with no cooldown of any kind, and an
                    // addon can see it (decision 1933's fold-back). The equipped-cooldown feed is
                    // still the INTERIM above; this is the half that now exists.
                    if inst.duration_ms.is_some() {
                        Value::Boolean(true)
                    } else {
                        Value::Nil
                    },
                    Value::Integer(0),
                ]))
            },
        )?,
    )?;

    // GameTooltip:SetBagItem(bag, slot) → hasCooldown, repairCost — the real-instance hover.
    // The one money-eligible source built so far: merchant open + repair off ⇒ the engine fires
    // OnTooltipAddMoney(SellPrice × stack), or prints ITEM_UNSELLABLE at price 0 (the gate at
    // `0x52e376`, engine-side at last). repairCost is 0 INTERIM (the per-item
    // durability/repair feed is the paper-doll arc's).
    m.set(
        "SetBagItem",
        lua.create_function(|lua, (this, bag, slot): (Table, i64, u32)| {
            let h = frame_handle_of(lua, &this)?;
            let (item_id, count, has_cd, link, quality, inst) = {
                let model = lua.app_data_mut::<Model>().expect("model app_data");
                match model
                    .containers
                    .get(&bag)
                    .and_then(|c| c.slots.get(&slot))
                    .filter(|s| s.item_id != 0)
                {
                    // The reference's own leg selector, byte-read: `SetBagItem 0x534620` asks the
                    // item-cooldown query `0x6e2ed0` and takes the p6=1 (instance-block) leg iff
                    // **all three** of enable/start/duration are non-zero — a genuinely running
                    // cooldown. That one boolean is both the Lua `hasCooldown` return and the
                    // openable gate's inverse, so they are computed once, here.
                    Some(s) => {
                        let has_cd = s
                            .cooldown
                            .is_some_and(|(start, dur, en)| en && start > 0 && dur > 0);
                        (
                            s.item_id,
                            s.count.max(1),
                            has_cd,
                            s.link.clone(),
                            s.quality.unwrap_or(1),
                            render::ItemInstance {
                                // The bag slot's display name rides its LINK (the reference
                                // builds that link out of `0x5d8b00`'s output, so the two are the
                                // same string by construction — the roll's suffix included).
                                name: s.link.as_deref().and_then(link_name).map(str::to_string),
                                durability: s.durability,
                                creator: s.creator.clone(),
                                has_text: s.readable,
                                flags: s.flags,
                                already_bound: s.already_bound,
                                // Line 3 — the charter's guild name and master. Only the BAG hover
                                // carries it: a charter has `InventoryType = 0` and cannot be worn,
                                // so the doll site above has no petition to show and does not ask
                                // its slot view for one.
                                petition: s.petition.clone(),
                                enchants: s.enchants.clone(),
                                // p6 == 0 ⇔ no running cooldown. A clam shows the green line; the
                                // same clam mid-cooldown would show ITEM_COOLDOWN_TIME instead
                                // (that line has no feed here yet — a separate, pre-existing gap).
                                openable_source: !has_cd,
                                duration_ms: s.duration_ms,
                            },
                        )
                    }
                    None => return Ok(MultiValue::from_vec(vec![Value::Nil])),
                }
            };
            {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                clear_content(&mut model, h);
            }
            fire_cleared(lua, h);
            match view_of(lua, item_id) {
                Some(v) => {
                    render_view(lua, &this, &v, BuilderFlags::default(), Some(&inst))?;
                    let (merchant_open, repairing) = {
                        let model = lua.app_data_mut::<Model>().expect("model app_data");
                        (model.merchant.is_some(), model.repair_mode)
                    };
                    if merchant_open && !repairing {
                        if v.sell_price > 0 {
                            fire_add_money(lua, h, u64::from(v.sell_price) * u64::from(count));
                        } else if let Some(t) = crate::strings::global(lua, "ITEM_UNSELLABLE") {
                            append_line(lua, &this, (t, WHITE), None, false)?;
                        }
                    }
                }
                None => {
                    // Template in flight. The real client early-outs to an EMPTY tooltip here
                    // (`0x52b6a3`); benilla keeps a name-only line instead
                    // (the name is already on the slot's link, and a blank plate
                    // under an on-screen name reads broken). The re-enter loop repaints the
                    // moment the push lands.
                    if let Some(name) = link.as_deref().and_then(link_name) {
                        append_line(
                            lua,
                            &this,
                            (name.to_string(), quality_color(quality)),
                            None,
                            false,
                        )?;
                    }
                }
            }
            show_or_hide_empty(lua, h);
            Ok(MultiValue::from_vec(vec![
                // Not just the cooldown: the builder's return is set by four line sites, and the
                // item's own duration line is one of them (see `SetInventoryItem`'s note above,
                // decision 1933's fold-back).
                Value::Boolean(has_cd || inst.duration_ms.is_some()),
                Value::Integer(0),
            ]))
        })?,
    )?;

    // GameTooltip:SetLootItem(slot) — the loot-row hover (the reference's own binding,
    // `0x533470`, which its `LootButtonTemplate` <OnEnter> runs behind `LootSlotIsItem`).
    // **A block source, not a template one** (the leg at `0x533564` supplies the instance block,
    // p6=1), and that is the whole point of it existing here: a loot
    // slot is no item object, so its rolled random-suffix enchants reach the builder through the
    // block — never through `ITEM_FIELD_ENCHANTMENT`, which the wire does not carry for loot.
    // Hovering a "… of the Monkey" drop through the template path `BenillaSetItemById` instead printed
    // the `<Random enchantment>` placeholder until the item was in the bag.
    //
    // `slot` is the 1-based display row, like every other loot getter; the coin pile and a
    // cleared row have no tooltip.
    // GameTooltip:SetTradePlayerItem(id) / SetTradeTargetItem(id) — `0x5341e0` / `0x534410`, two
    // arguments exact (self, slot), no returns: the trade slot's item, rendered off its template
    // the way the loot slot is (the stock TradeFrame.xml's slot OnEnter). The
    // target side reads the partner's slot guids (`[0xb715a0 + slot*4]`); an empty or
    // out-of-range slot leaves the tooltip untouched. The
    // slot's enchant line waits on the trade snapshot carrying the wire's enchant id (0592 P3).
    for (name, player_side) in [("SetTradePlayerItem", true), ("SetTradeTargetItem", false)] {
        m.set(
            name,
            lua.create_function(move |lua, (this, slot): (Table, usize)| {
                let h = frame_handle_of(lua, &this)?;
                let item = {
                    let model = lua.app_data_ref::<Model>().expect("model app_data");
                    model.trade.as_ref().and_then(|t| {
                        let side = if player_side { &t.player } else { &t.target };
                        slot.checked_sub(1)
                            .and_then(|n| side.slots.get(n))
                            .and_then(|s| s.clone())
                    })
                };
                let Some(item) = item.filter(|i| i.item_id != 0) else {
                    return Ok(());
                };
                {
                    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                    clear_content(&mut model, h);
                }
                fire_cleared(lua, h);
                let inst = render::ItemInstance {
                    name: item.name.clone(),
                    ..Default::default()
                };
                match view_of(lua, item.item_id) {
                    Some(v) => render_view(lua, &this, &v, BuilderFlags::default(), Some(&inst))?,
                    None => {
                        if let Some(name) = item.name.clone() {
                            append_line(
                                lua,
                                &this,
                                (name, quality_color(item.quality.unwrap_or(1))),
                                None,
                                false,
                            )?;
                        }
                    }
                }
                show_or_hide_empty(lua, h);
                Ok(())
            })?,
        )?;
    }

    m.set(
        "SetLootItem",
        lua.create_function(|lua, (this, slot): (Table, usize)| {
            let h = frame_handle_of(lua, &this)?;
            let row = {
                let model = lua.app_data_mut::<Model>().expect("model app_data");
                model
                    .loot
                    .as_ref()
                    .and_then(|l| slot.checked_sub(1).and_then(|n| l.rows.get(n)))
                    .and_then(|r| r.clone())
                    .filter(|r| !r.is_coin && r.item_id != 0)
            };
            let Some(row) = row else {
                return Ok(());
            };
            {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                clear_content(&mut model, h);
            }
            fire_cleared(lua, h);
            // The row's own name is the app-composed one — the roll's suffix already joined, so
            // the plate reads "Chipped Claw of the Bear" like the reference's `0x5d8b00` output.
            // The lines come from the roll id through the pushed table, which is where the
            // reference reads them too (its `+0x424` against the DBC store, `0x52b7bf`).
            let inst = render::ItemInstance {
                name: row.name.clone(),
                enchants: roll_enchants(lua, row.random_property_id),
                ..Default::default()
            };
            match view_of(lua, row.item_id) {
                Some(v) => render_view(lua, &this, &v, BuilderFlags::default(), Some(&inst))?,
                // Template in flight — the row's name holds the plate (SetBagItem's 0138 posture;
                // the re-enter loop repaints when the push lands).
                None => {
                    if let Some(name) = row.name.clone() {
                        append_line(
                            lua,
                            &this,
                            (name, quality_color(row.quality.unwrap_or(1))),
                            None,
                            false,
                        )?;
                    }
                }
            }
            show_or_hide_empty(lua, h);
            Ok(())
        })?,
    )?;

    // GameTooltip:SetLootRollItem(rollId) — the group-loot roll window's hover (the reference's
    // own `0x5364a0`). The same shape as SetLootItem beside it, byte-verified on the same
    // dispatch: p6=1 with `+0x424 ← [roll+0x20]` (the roll's randomPropertyId), all 7 enchant
    // slots zero, and BOTH guid args `&{0,0}` — no item object, so the suffix-row copy (`0x52b7bf`)
    // is the only enchant source and the placeholder arm (`0x52cc33`) is unreachable.
    m.set(
        "SetLootRollItem",
        lua.create_function(|lua, (this, roll_id): (Table, u32)| {
            let h = frame_handle_of(lua, &this)?;
            let entry = {
                let model = lua.app_data_mut::<Model>().expect("model app_data");
                model
                    .loot_rolls
                    .rolls
                    .iter()
                    .find(|r| r.roll_id == roll_id)
                    .cloned()
                    .filter(|r| r.item_id != 0)
            };
            let Some(entry) = entry else {
                return Ok(());
            };
            {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                clear_content(&mut model, h);
            }
            fire_cleared(lua, h);
            let inst = render::ItemInstance {
                name: entry.name.clone(),
                enchants: roll_enchants(lua, entry.random_property_id),
                ..Default::default()
            };
            match view_of(lua, entry.item_id) {
                Some(v) => render_view(lua, &this, &v, BuilderFlags::default(), Some(&inst))?,
                None => {
                    if let Some(name) = entry.name.clone() {
                        append_line(
                            lua,
                            &this,
                            (name, quality_color(entry.quality.unwrap_or(1))),
                            None,
                            false,
                        )?;
                    }
                }
            }
            show_or_hide_empty(lua, h);
            Ok(())
        })?,
    )?;

    // GameTooltip:SetMerchantCompareItem(index [, offset]) — `0x536080`, GameTooltip method-table
    // entry 43. The shopping tooltip the stock vendor row raises beside the main one: "here is what
    // you are wearing that this would replace".
    //
    // **`offset` is not a slot.** It is the 1-based ordinal among CANDIDATE slots, and a slot is a
    // candidate only when all three of these hold — the reference's own conjunction:
    //
    //   1. its bit is set in `0x809200[InventoryType]` (our [`equip_slots_for`]),
    //   2. the slot is OCCUPIED,
    //   3. the equipped item's **class** equals the vendor item's class. Class only — subclass is
    //      never read.
    //
    // Empty slots are skipped **without** decrementing, so offset 1 finds the first occupied
    // matching slot wherever it sits. The consequences are the reference's and they are not
    // obvious: a shield in the off hand makes offset 2 nil for a one-hand weapon (ARMOR ≠ WEAPON),
    // and an equipped wand DOES compare against a bow on the shelf (both class 2, both bit 17).
    // Any type with one candidate slot answers nil for offset 2, which is what makes stock's
    // second `if` cheap.
    //
    // **The return contract decides ghost tooltips versus none**, so it is transcribed exactly:
    // ONE Lua value on every non-raising path — the NUMBER 1 on success (`lua_pushnumber`, not a
    // boolean), `nil` on every failure. A bad `this` or a non-number index RAISES; a missing
    // `offset` does not, and defaults to 1. An explicit `offset` of 0 or less is nil, because the
    // counter starts below zero and only ever decrements.
    //
    // An uncached vendor template answers nil AND fires the query ([`view_of`] does both), so the
    // first hover of an unseen vendor item shows no shopping tooltip and a later one does. That is
    // the reference's behaviour, not a race we lost.
    m.set(
        "SetMerchantCompareItem",
        lua.create_function(
            |lua, (this, index, offset): (Table, Value, Option<Value>)| {
                let h = frame_handle_of(lua, &this)?;
                let as_number = |v: &Value| -> Option<f64> {
                    match v {
                        Value::Integer(i) => Some(*i as f64),
                        Value::Number(n) => Some(*n),
                        // `lua_isnumber` accepts a numeric string, here as everywhere.
                        Value::String(s) => s.to_str().ok().and_then(|s| s.parse::<f64>().ok()),
                        _ => None,
                    }
                };
                // The one argument whose absence is an ERROR, with the client's own usage text.
                let Some(idx) = as_number(&index) else {
                    return Err(mlua::Error::runtime(
                        "Usage: SetMerchantCompareItem(\"slot\" [, offset])",
                    ));
                };
                // The reference re-bases the index in f64 and THEN truncates toward zero, so 2.7
                // is row 1 (1.7 → 1) rather than row 2. Doing it the other way round differs by
                // one for every fractional index.
                let row = (idx - 1.0).trunc();
                if row.is_nan() || row < 0.0 || row > f64::from(u32::MAX) {
                    return Ok(Value::Nil);
                }
                // `offset` re-bases as an INTEGER, and its absence means 1.
                let left = match offset.as_ref().and_then(as_number) {
                    Some(n) if n.is_nan() => return Ok(Value::Nil),
                    Some(n) => n.trunc() - 1.0,
                    None => 0.0,
                };
                if left < 0.0 {
                    return Ok(Value::Nil);
                }

                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let row = row as usize;
                let Some(item_id) = ({
                    let model = lua.app_data_ref::<Model>().expect("model app_data");
                    model
                        .merchant
                        .as_ref()
                        .and_then(|m| m.items.get(row))
                        .map(|it| it.item_id)
                }) else {
                    return Ok(Value::Nil);
                };
                // The vendor item's own template — the class this compares by, and the miss that
                // fires the query.
                let Some(offered) = view_of(lua, item_id) else {
                    return Ok(Value::Nil);
                };

                compare_against_worn(lua, &this, h, &offered, left)
            },
        )?,
    )?;

    // GameTooltip:SetAuctionCompareItem("type", index [, offset]) — `0x535d70`, the auction row's
    // twin of the vendor compare: the same body (`0x53603e` and `0x5362d4` both reach the shared
    // compare with `p4 = 0, p5 = 1`), fed by one of the three auction lists instead of the vendor's
    // shelf.
    // The type argument is `lua_isstring`-gated (`0x535e28`) and matched against the list names;
    // a non-string type or non-number index raises the reference's own Usage; a bad list name,
    // an out-of-range row or an uncached template is nil. Return contract as the merchant's: the
    // NUMBER 1 on success, nil otherwise — the stock `AuctionFrameItem_OnEnter` gates each
    // shopping tooltip on it (1971).
    m.set(
        "SetAuctionCompareItem",
        lua.create_function(
            |lua, (this, kind, index, offset): (Table, Value, Value, Option<Value>)| {
                let h = frame_handle_of(lua, &this)?;
                let as_number = |v: &Value| -> Option<f64> {
                    match v {
                        Value::Integer(i) => Some(*i as f64),
                        Value::Number(n) => Some(*n),
                        Value::String(s) => s.to_str().ok().and_then(|s| s.parse::<f64>().ok()),
                        _ => None,
                    }
                };
                let kind = match &kind {
                    Value::String(s) => s.to_str()?.to_string(),
                    Value::Integer(i) => i.to_string(),
                    Value::Number(n) => n.to_string(),
                    _ => {
                        return Err(mlua::Error::runtime(
                            "Usage: SetAuctionCompareItem(\"type\", index [, offset])",
                        ))
                    }
                };
                let Some(idx) = as_number(&index) else {
                    return Err(mlua::Error::runtime(
                        "Usage: SetAuctionCompareItem(\"type\", index [, offset])",
                    ));
                };
                let row = (idx - 1.0).trunc();
                if row.is_nan() || row < 0.0 || row > f64::from(u32::MAX) {
                    return Ok(Value::Nil);
                }
                let left = match offset.as_ref().and_then(as_number) {
                    Some(n) if n.is_nan() => return Ok(Value::Nil),
                    Some(n) => n.trunc() - 1.0,
                    None => 0.0,
                };
                if left < 0.0 {
                    return Ok(Value::Nil);
                }
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let row = row as usize;
                let Some(item_id) = ({
                    let model = lua.app_data_ref::<Model>().expect("model app_data");
                    let list = super::auction::list_index_of(&kind);
                    model
                        .auction
                        .as_ref()
                        .zip(list)
                        .and_then(|(a, l)| a.lists[l].rows.get(row))
                        .map(|r| r.item_id)
                }) else {
                    return Ok(Value::Nil);
                };
                let Some(offered) = view_of(lua, item_id) else {
                    return Ok(Value::Nil);
                };
                compare_against_worn(lua, &this, h, &offered, left)
            },
        )?,
    )?;

    // GameTooltip:SetMerchantItem(index) / SetBuybackItem(index) — template-id sources through
    // the same store (the real `SetMerchantItem 0x534080` is the template path); the
    // merchant feed's own stat head is the fallback while the template answer is in flight, so
    // the vendor hover always shows at least the head it showed pre-0274.
    for (method, buyback) in [("SetMerchantItem", false), ("SetBuybackItem", true)] {
        m.set(
            method,
            lua.create_function(move |lua, (this, index): (Table, usize)| {
                let h = frame_handle_of(lua, &this)?;
                let row = {
                    let model = lua.app_data_mut::<Model>().expect("model app_data");
                    let Some(merchant) = &model.merchant else {
                        return Ok(());
                    };
                    let list = if buyback {
                        &merchant.buyback
                    } else {
                        &merchant.items
                    };
                    list.get(index.saturating_sub(1)).cloned()
                };
                let Some(row) = row else { return Ok(()) };
                {
                    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                    clear_content(&mut model, h);
                }
                fire_cleared(lua, h);
                match view_of(lua, row.item_id) {
                    Some(v) => {
                        render_view(lua, &this, &v, BuilderFlags::default(), None)?;
                        // **The vendor tab arms nothing** — `MerchantFrame.xml:67-80` seats
                        // `ShoppingTooltip1/2` itself on every hover of it, unconditionally, and
                        // that is the reference's own live compare. Arming here would put the
                        // shift drive on the same two plates the stock row already owns, and its
                        // release edge would then hide a compare the player never asked it to.
                        // The BUYBACK tab is the other branch of the same button and seats
                        // nothing, so it arms like any other hover.
                        if buyback {}
                    }
                    None => {
                        // Template in flight: the row's own stat head as a minimal view.
                        let head = row.stats.unwrap_or_default();
                        let v = ItemTemplateView {
                            name: row.name.clone().unwrap_or_default(),
                            quality: head.quality,
                            class: head.class,
                            subclass: head.subclass,
                            inventory_type: head.inventory_type,
                            damages: if head.dmg_max > 0.0 {
                                vec![(head.dmg_min, head.dmg_max, head.dmg_type)]
                            } else {
                                Vec::new()
                            },
                            delay_ms: head.delay_ms,
                            armor: head.armor,
                            block: head.block,
                            ..Default::default()
                        };
                        render_view(lua, &this, &v, BuilderFlags::default(), None)?;
                    }
                }
                show_or_hide_empty(lua, h);
                Ok(())
            })?,
        )?;
    }

    // GameTooltip:SetInboxItem(index) — the mail window's enclosed-item hover: the
    // inbox row's item entry through the same id-keyed store (MailFrame.lua l.218/470). A row with no
    // enclosed item (item_id 0) is a no-op, like the reference (which only calls this when hasItem).
    m.set(
        "SetInboxItem",
        lua.create_function(|lua, (this, index): (Table, usize)| {
            let h = frame_handle_of(lua, &this)?;
            let (item_id, roll, name) = {
                let model = lua.app_data_mut::<Model>().expect("model app_data");
                let Some(mail) = &model.mail else {
                    return Ok(());
                };
                match mail.inbox.get(index.saturating_sub(1)) {
                    Some(r) => (r.item_id, r.item_random_property_id, r.item_name.clone()),
                    None => (0, 0, None),
                }
            };
            if item_id == 0 {
                return Ok(());
            }
            {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                clear_content(&mut model, h);
            }
            fire_cleared(lua, h);
            // A block source (`SetInboxItem 0x5355fa`, p6=1) carrying the attachment's roll — so
            // an "… of the Monkey" in the mail reads like the same drop in the loot window, and
            // the placeholder arm is unreachable here too.
            let inst = render::ItemInstance {
                name,
                enchants: roll_enchants(lua, roll),
                ..Default::default()
            };
            if let Some(v) = view_of(lua, item_id) {
                render_view(lua, &this, &v, BuilderFlags::default(), Some(&inst))?;
            }
            show_or_hide_empty(lua, h);
            Ok(())
        })?,
    )?;

    // GameTooltip:SetAuctionItem(type, index) — an auction ROW's hover. The row's
    // item entry through the same id-keyed store, keyed by list type ("list"/"bidder"/"owner").
    //
    // An auction row's tooltip deliberately emits **no** "Made by", "Gift from" or openable lines,
    // and the reason is structural rather than a rule anyone applies: the reference's binding
    // zeroes the GUID arguments (`0x535a6d`/`0x535a73`) before it resolves anything, so the gate
    // those lines hang off fails outright. Our id-keyed store has no instance to report either,
    // so we land on the same output by the same shape of reasoning.
    m.set(
        "SetAuctionItem",
        lua.create_function(|lua, (this, kind, index): (Table, String, usize)| {
            let h = frame_handle_of(lua, &this)?;
            let (item_id, roll, name) = {
                let model = lua.app_data_mut::<Model>().expect("model app_data");
                let Some(auction) = &model.auction else {
                    return Ok(());
                };
                let Some(list) = super::auction::list_index_of(&kind) else {
                    return Ok(());
                };
                match auction.lists[list].rows.get(index.saturating_sub(1)) {
                    Some(r) => (r.item_id, r.random_property_id, r.name.clone()),
                    None => (0, 0, None),
                }
            };
            if item_id == 0 {
                return Ok(());
            }
            {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                clear_content(&mut model, h);
            }
            fire_cleared(lua, h);
            // A block source (`SetAuctionItem 0x5359d9`, p6=1) carrying the listing's roll: a
            // rolled auction shows its real lines, never the placeholder. The
            // zeroed GUIDs the note above describes are unaffected — a roll is not an instance.
            let inst = render::ItemInstance {
                name,
                enchants: roll_enchants(lua, roll),
                ..Default::default()
            };
            if let Some(v) = view_of(lua, item_id) {
                render_view(lua, &this, &v, BuilderFlags::default(), Some(&inst))?;
            }
            show_or_hide_empty(lua, h);
            Ok(())
        })?,
    )?;

    // GameTooltip:SetAuctionSellItem() — the create-auction slot's hover. The item
    // staged in the sell slot, through the same id-keyed store. A no-op when the slot is empty
    // (the reference gates the call on GetAuctionSellItemInfo()).
    //
    // Unlike an auction ROW, this one names a real item the player owns — the reference passes the
    // live sell-slot GUID (`[0xb72608]`/`[0xb7260c]`) here and resolves an actual object
    // (`0x5357b4`), so its creator line is structurally reachable. Ours is still the id-keyed
    // template view, so we do not print one; that is a known narrowing, not a claim about the
    // reference.
    m.set(
        "SetAuctionSellItem",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let item_id = {
                let model = lua.app_data_mut::<Model>().expect("model app_data");
                model
                    .auction_sell_item
                    .as_ref()
                    .map(|it| it.item_id)
                    .unwrap_or(0)
            };
            if item_id == 0 {
                return Ok(());
            }
            {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                clear_content(&mut model, h);
            }
            fire_cleared(lua, h);
            if let Some(v) = view_of(lua, item_id) {
                render_view(lua, &this, &v, BuilderFlags::default(), None)?;
            }
            show_or_hide_empty(lua, h);
            Ok(())
        })?,
    )?;

    // GameTooltip:SetSendMailItem() — the mail Send tab's attached-item hover: the
    // cursor item attached to the send slot, through the same id-keyed store (MailFrame.lua l.952).
    // A no-op when nothing is attached (the reference gates the call on GetSendMailItem()).
    m.set(
        "SetSendMailItem",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let item_id = {
                let model = lua.app_data_mut::<Model>().expect("model app_data");
                model
                    .mail_send_item
                    .as_ref()
                    .map(|it| it.item_id)
                    .unwrap_or(0)
            };
            if item_id == 0 {
                return Ok(());
            }
            {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                clear_content(&mut model, h);
            }
            fire_cleared(lua, h);
            if let Some(v) = view_of(lua, item_id) {
                render_view(lua, &this, &v, BuilderFlags::default(), None)?;
            }
            show_or_hide_empty(lua, h);
            Ok(())
        })?,
    )?;

    // GameTooltip:SetTradeSkillItem(skillIndex [, reagentIndex]) — the tradeskill window's item
    // hover (decision 0437 phase 2): with reagentIndex, that recipe's reagent's item; without, the
    // recipe's PRODUCT item. Routes through the same id-keyed renderer BenillaSetItemById uses. A product
    // id of 0 (a pure-effect recipe) or an in-flight ask-once answer both fall to render_by_id's own
    // name-only fallback line (the recipe's name for the product channel, the reagent's own —
    // possibly still-nil — name for the reagent channel) rather than a no-op: that fallback already
    // exists and reads better than a blank hover. An out-of-range skill/reagent index is a no-op.
    m.set(
        "SetTradeSkillItem",
        lua.create_function(
            |lua, (this, skill_index, reagent_index): (Table, usize, Option<usize>)| {
                let found = {
                    let model = lua.app_data_ref::<Model>().expect("model app_data");
                    // The VISIBLE-row mapping (headers interleave since the grouped build,
                    // `0x4fca20`, landed) — never a raw recipes[] index; a header index resolves to
                    // None.
                    let Some(recipe) = super::tradeskill::recipe_at(&model, skill_index) else {
                        return Ok(());
                    };
                    match reagent_index {
                        Some(ri) => ri
                            .checked_sub(1)
                            .and_then(|i| recipe.reagents.get(i))
                            .map(|r| (r.item, r.name.clone())),
                        None => Some((recipe.product_item, Some(recipe.name.clone()))),
                    }
                };
                let Some((item_id, fb_name)) = found else {
                    return Ok(());
                };
                render_by_id(lua, &this, item_id, fb_name, None)
            },
        )?,
    )?;

    // GameTooltip:SetCraftItem(craftIndex, reagentIndex) — the craft window's reagent hover
    // (decision 0437 phase 3, ref `CraftItemTemplate`'s OnEnter, `Blizzard_CraftUI.xml:33`):
    // ALWAYS the REAGENT's item — unlike SetTradeSkillItem, Craft has no product-item concept to
    // fall back to when reagentIndex is omitted (craft.rs's own module doc, "no product-item
    // concept"), so this signature has no optional arm; the ref itself never calls it with one
    // argument either. Routes through the same id-keyed renderer. An out-of-range craft/reagent
    // index is a no-op.
    m.set(
        "SetCraftItem",
        lua.create_function(
            |lua, (this, craft_index, reagent_index): (Table, usize, usize)| {
                let found = {
                    let model = lua.app_data_ref::<Model>().expect("model app_data");
                    let Some(c) = &model.craft else {
                        return Ok(());
                    };
                    let Some(recipe) = craft_index.checked_sub(1).and_then(|i| c.recipes.get(i))
                    else {
                        return Ok(());
                    };
                    reagent_index
                        .checked_sub(1)
                        .and_then(|i| recipe.reagents.get(i))
                        .map(|r| (r.item, r.name.clone()))
                };
                let Some((item_id, fb_name)) = found else {
                    return Ok(());
                };
                render_by_id(lua, &this, item_id, fb_name, None)
            },
        )?,
    )?;

    Ok(())
}
