//! The loot bindings — the Era-shaped loot-window surface, the same two-way seam as
//! [`super::merchant`]/[`super::container`]/[`super::gossip`]: the app pushes a **loot snapshot**
//! ([`UiScript::set_loot`] — the rows already resolved from the wire to name/icon/quantity/quality by
//! the app's item stores, with the synthesized coin row first) and the Lua `LootSlot`/`CloseLoot`
//! calls queue outbound **intents** the app drains ([`UiScript::take_loot_picks`] /
//! [`UiScript::take_loot_close`]). The engine holds no loot knowledge — a row is "a name, an icon
//! path, a quantity, a quality, and whether it's the coin pile".
//!
//! ## The Era API shape
//!
//! 1.12's loot API is a flat set of globals the FrameXML `LootFrame.lua` drives (VERIFIED against the
//! extracted `LootFrame.lua`, scratchpad): `GetNumLootItems()`, `GetLootSlotInfo(slot)` →
//! `texture, item, quantity, quality` (`LootFrame.lua:81`), `LootSlotIsItem(slot)` /
//! `LootSlotIsCoin(slot)` (`LootFrame.lua:80`), `GetLootSlotLink(slot)` → the row's item link (what
//! the row click's ctrl/shift arms read, `LootFrame.lua:149`/`:152`),
//! and `CloseLoot()` (fired from `OnHide`, `LootFrame.lua:143-145`). `slot` is **1-based**; an
//! out-of-range slot answers `nil`.
//!
//! ## The take is TWO verbs, not one
//!
//! `LootSlot(slot)` looks like "loot row n" and is not: in 1.12 it is the **LOOT_BIND confirmation
//! continuation** alone. The C dispatcher `0x4c2790(slot, flag)` takes the row only on `flag == 0`,
//! which no Lua binding reaches — the row click is the C `CLootButton`'s own behaviour — while
//! `LootSlot 0x4c2e70` passes `flag = 1`, whose arm refuses every slot but the one a bind confirm
//! is pending for. benilla builds that widget — [`crate::widget::FrameKind::LootButton`], decision
//! 1799 — so the row click's take runs where the reference runs it, in `script::button`'s click
//! path under the same no-modifier gate; `BenillaTakeLootSlot` is the Lua-reachable entry to the
//! same queue, and `LootSlot` keeps the reference's meaning exactly.
//!
//! The **coin pile is a synthesized client-side row** (first in the list when the loot carries gold):
//! `LootSlotIsCoin` is true for it, its `item` text is the formatted money amount, and `LootSlot(1)`
//! on it queues the money intent. The app maps a clicked 1-based row to either the money intent or the
//! wire loot slot the item lives at (`CMSG_AUTOSTORE_LOOT_ITEM` addresses the **wire** slot, which is
//! *not* the 1-based display position once a coin row is prepended or a row is removed) — the Lua side
//! never sees the wire slot, exactly as the merchant's Lua side never sees the item entry.
//!
//! ## Master loot
//!
//! Two more globals ride the same seam when the group's loot method is master loot:
//! `GetMasterLootCandidate(index)` reads [`LootState::master_candidates`] (names, 1-based, dense)
//! and `GiveMasterLoot(slot, candidateIndex)` queues an assignment the app resolves to a wire slot
//! and a recipient guid. The *decision* to open the dropdown is not Lua's: the app fires
//! `OPEN_MASTER_LOOT_LIST` when a picked row's wire `slot_type` is `MASTER`, which is where the
//! real client puts it too — its take dispatcher branches on the same byte before any Lua runs.

use mlua::{Lua, MultiValue, Table, Value};

use super::binding_abi::flag;
use super::Model;

/// `CLootButton`'s own Lua method table (`0x847ce4`) — see [`crate::widget::FrameKind::LootButton`].
/// Exactly one entry, and the count is read off the registrar's `mov edx,1` rather than off a run
/// length, so it cannot drift.
pub(super) const REG_LOOTBUTTON_METHODS: &str = "__benilla_lootbutton_methods";

/// The **item-cache miss** quality `GetLootSlotInfo` answers for a row whose item template has not
/// landed yet — the reference's own sentinel, and emphatically **not** a nil. `0x4c23a0` reads the
/// item-cache record's `[rec+0x1c]` and hands back `-1` when the cache has no record for the id.
///
/// It is why stock `UIParent.lua` builds `ITEM_QUALITY_COLORS` over **`for i = -1, 6`** (l.66):
/// index `-1` exists *for this value*. `LootFrame_Update` indexes the table with the raw return and
/// no guard — `color = ITEM_QUALITY_COLORS[quality]` (`LootFrame.lua:82`), dereferenced one line
/// later at `color.r` (`:85`) — so a nil here is a Lua runtime error on every loot opened before
/// its templates arrive, which is every loot on a cold item cache.
const CACHE_MISS_QUALITY: i64 = -1;

/// The row text on that same miss. The reference composes every loot row's name through `0x5d8b00`,
/// which leaves its destination buffer **empty** when the item cache has no record (`0x5d8b25`
/// stores the terminator and returns); the binding then pushes that empty string. Four values, one
/// of them `""` — never three values, and never a nil.
const CACHE_MISS_NAME: &str = "";

/// The icon on an `ItemDisplayInfo` miss — a row that HAS an item, whose display id the catalog has
/// no icon for. The reference appends the literal `INV_Misc_QuestionMark` (`0x847fe4`, `0x4c252b`)
/// to the same `StringLookups.dbc` row-3 prefix (`Interface\\Icons`) a hit uses.
///
/// It is deliberately **not** the answer for a slot with no item: `0x4c2460` returns NULL on each of
/// its three guards (`0x4c2470` no window, `0x4c24a6` out of range, `0x4c24b7` itemId == 0) and
/// `lua_pushstring 0x6f3890` tail-jumps a NULL to `lua_pushnil`. The question mark needs a live
/// itemId to be reached at all.
const MISSING_ICON: &str = "Interface\\Icons\\INV_Misc_QuestionMark";

/// The quality a slot with **no item at all** answers — a slot past the end, Lua slot 0, or any slot
/// while no loot window is open. `0x4c23a0` returns 0 on each of its three guards, and the binding
/// `fild`s that. Distinct from [`CACHE_MISS_QUALITY`]: a **cleared** slot still has a record, whose
/// itemId is now 0, and `0x55ba30` short-circuits an entryId of 0 to a NULL record
/// (`0x55ba3d`/`0x55ba42`) — a guaranteed cache miss, so it takes the `-1` arm at `0x4c2435`.
const NO_SLOT_QUALITY: i64 = 0;

/// One loot-window row, resolved by the app. Plain data — its 1-based order in the
/// window is its position in [`LootState::rows`]; the coin row (when present) is always position 1.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LootRow {
    /// The row text (`GetLootSlotInfo`'s `item` return): an item's name, or the coin row's formatted
    /// money amount. `None` while the ask-once item-template query is still in flight (items only —
    /// the coin row's text is always set), and the API reports [`CACHE_MISS_NAME`] — the empty
    /// string the reference's own name formatter leaves behind on a cache miss — never a nil.
    pub name: Option<String>,
    /// Icon texture path (`Interface\Icons\…` — from the wire `display_info_id`, or the coin
    /// texture). `None` only if the display catalog had no icon for the id, which the API reports as
    /// [`MISSING_ICON`], the reference's own `INV_Misc_QuestionMark`.
    pub texture: Option<String>,
    /// The stack size looted (`GetLootSlotInfo`'s `quantity`); the coin row is always `1`.
    pub quantity: u32,
    /// Item quality 0..6, for the quality-coloured row text; `None` while the template is in flight,
    /// which the API reports as [`CACHE_MISS_QUALITY`] (`-1`, the reference's cache-miss sentinel and
    /// a real row of `ITEM_QUALITY_COLORS`) — never a nil, which is what raised in the stock
    /// `LootFrame_Update`. The coin row carries a fixed quality.
    pub quality: Option<u32>,
    /// Whether this is the synthesized coin pile (`LootSlotIsCoin` true, `LootSlotIsItem` false).
    pub is_coin: bool,
    /// The item id — the shared item-tooltip store's key (`BenillaGetItemStats`); `0` for the coin
    /// row. A benilla extension riding as a TRAILING return of `GetLootSlotInfo` (the era 4-tuple
    /// never carried it; tooltip content was C++'s alone), same idiom as the quest item getters.
    pub item_id: u32,
    /// The row's full escaped `|cff…|Hitem:…|h[Name]|h|r` link (`GetLootSlotLink`).
    /// `None` for the synthesized coin row (there is no item to link) and while the ask-once
    /// template answer is in flight — the link embeds the name, so it cannot exist before `name`
    /// does; the same `Option` shape [`super::char_stats::InvSlotView::link`] carries. Both arms of
    /// the STOCK row click take a nil in stride: `DressUpItemLink` returns on one (its own guard,
    /// `DressUpFrame.lua:10-16`), and `ChatFrameEditBox:Insert` takes an `Option<String>`
    /// ([`super::editbox`]) so the other drops it silently.
    pub link: Option<String>,
    /// The wire's `randomPropertyId` — the drop's **random-suffix roll**, the id the tooltip
    /// resolves against [`super::Model::random_properties`] for its enchant lines.
    /// `0` = unrolled.
    ///
    /// The client keeps exactly this, at `+0x14` of its own 0x1c-byte loot record, and
    /// `SetLootItem 0x533470` copies it into the tooltip's `+0x424` — a loot slot is **not** an
    /// item object (that leg passes an all-zero item guid), so the roll is the only enchant source
    /// a loot hover can have. [`Self::name`] already carries the suffix the same id joins on.
    pub random_property_id: u32,
}

/// One open loot window: its rows (coin first when present). Pushed whole by the app; `None` means no
/// loot is open (the window is closed).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LootState {
    /// The window's **fixed** slot list: positions never shift while the window is open. A looted
    /// slot becomes `None` — still counted by `GetNumLootItems` (the reference's slot array keeps
    /// cleared slots; `LootFrame.numLootItems` is captured once at `OnShow`, l.132) but neither
    /// `LootSlotIsItem` nor `LootSlotIsCoin`, which is exactly how the reference's
    /// `LootFrame_Update` leaves a cleared slot's button hidden in place (`LootFrame.lua:80`)
    /// instead of collapsing the rows below it upward.
    pub rows: Vec<Option<LootRow>>,
    /// Whether this loot came from fishing (`IsFishingLoot()` — the wire `SMSG_LOOT_RESPONSE`
    /// `loot_type == 3`). `LootFrame_OnShow` keys the "FISHING REEL IN" sound and
    /// the FishingLoot portrait overlay on it (`LootFrame.lua:137-140`).
    pub fishing: bool,
    /// The master-loot candidate slots — what `GetMasterLootCandidate(i)` answers for a 1-based
    /// `i`. Empty unless the group's loot method is master loot and the server
    /// sent `SMSG_LOOT_MASTER_LIST` for this window.
    ///
    /// **A `None` is a real slot that answers nil**, not padding, and the list is deliberately
    /// not packed: in a raid the client files each candidate into its own subgroup's five-slot
    /// block, so index `i` carries which raid group the candidate is in. That is what lets
    /// `GroupLootDropDown_Initialize` walk `1..40` in blocks of five and label each block
    /// "Group N" (`LootFrame.lua:186-212`). A `None` also covers a candidate whose name has not
    /// resolved yet — the binding pushes nil for that too, and `UPDATE_MASTER_LOOT_LIST` exists
    /// to repaint the menu when it lands.
    ///
    /// Names, not guids: the seam's standing division is that Lua speaks names and 1-based
    /// indices while the app owns guids and wire slots, and `GroupLootDropDown_Initialize` puts
    /// this string straight into `info.text` (`LootFrame.lua:181`/`:229`).
    pub master_candidates: Vec<Option<String>>,
}

impl super::UiScript {
    /// Push (or clear, with `None`) the open loot's row snapshot.
    pub fn set_loot(&mut self, state: Option<LootState>) {
        self.model_mut().loot = state;
    }

    /// Drain the 1-based row indices queued by `BenillaTakeLootSlot` since the last call — the ROW
    /// CLICK's take. The app maps each to either the coin (money) intent or the item's wire loot
    /// slot, and applies the bind-on-pickup deferral.
    pub fn take_loot_picks(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().loot_picks)
    }

    /// Drain the 1-based row indices queued by `LootSlot` — the LOOT_BIND confirmation
    /// continuations. Each is honoured only if it names the row the app is actually holding a
    /// confirm open for (the reference's `[0x847cec]` gate); anything else is dropped.
    pub fn take_loot_confirms(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().loot_confirms)
    }

    /// Whether `CloseLoot` was called since the last drain (and clear the flag). The app maps this to
    /// `CMSG_LOOT_RELEASE` when a loot is open.
    pub fn take_loot_close(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().loot_close)
    }

    /// Drain the `GiveMasterLoot(slot, candidateIndex)` assignments queued since the last call —
    /// both 1-based display numbers. The app resolves the row to its wire slot and the candidate
    /// to a guid, then sends `CMSG_LOOT_MASTER_GIVE`.
    pub fn take_loot_master_gives(&mut self) -> Vec<(u32, u32)> {
        std::mem::take(&mut self.model_mut().loot_master_gives)
    }
}

/// Register the loot globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetNumLootItems() → the number of slots the open loot has, cleared ones included (0 when none
    // is open). Constant while a window is open — the reference reads it once at OnShow (l.132) and
    // never again; ours is invariant by construction, so the live read is the same value.
    g.set(
        "GetNumLootItems",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.loot.as_ref().map_or(0, |l| l.rows.len()) as i64)
        })?,
    )?;

    // GetLootSlotInfo(slot) → texture, item, quantity, quality — the reference's four, in its order
    // (`0x4c2c60`, `mov eax,4` at `0x4c2d07`; `LootFrame.lua:81`). `slot` is 1-based.
    //
    // **It answers four values on every leg — never three, never a bare nil**.
    // A slot with no item at all (past the end, slot 0, no window) is `nil, "", 0, 0`; a CLEARED
    // slot is `nil, "", 0, -1`, because its record survives with a zeroed itemId and that is a
    // guaranteed item-cache miss.
    //
    // A LIVE row is the same story one level down: each producer has a sentinel for the case where
    // the value it wants is not resolved yet — [`CACHE_MISS_QUALITY`] (`-1`) and [`CACHE_MISS_NAME`]
    // (`""`) while the item template is in flight, [`MISSING_ICON`] when the display catalog has no
    // icon for the wire display id. The quality is the load-bearing one: the reference's
    // `ITEM_QUALITY_COLORS` has a `-1` row precisely so this value can index it.
    //
    // The in-flight row is nonetheless one the REFERENCE never paints, because it does not open the
    // window until every template has landed ([`crate::script`] has no say in that; the app's
    // `ui_loot` holds `LOOT_OPENED` back). These sentinels are the floor under that, not the plan.
    g.set(
        "GetLootSlotInfo",
        lua.create_function(|lua, slot: usize| {
            // `Some(Some(row))` is a live row; `Some(None)` is a CLEARED one (the slot survives with
            // its record zeroed); `None` is no such slot at all.
            let slot_state = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model
                    .loot
                    .as_ref()
                    .and_then(|l| slot.checked_sub(1).and_then(|n| l.rows.get(n)))
                    .cloned()
            };
            let Some(Some(row)) = slot_state else {
                // No row — and the reference still pushes four (`0x4c2d07 mov eax,4` is
                // unconditional; there is no short-return path). A cleared slot's record has a
                // zeroed itemId, which `0x55ba30` short-circuits to a NULL cache record, so it takes
                // the cache-miss quality where a genuinely absent slot takes the guard's zero.
                let cleared = slot_state.is_some();
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::String(lua.create_string(CACHE_MISS_NAME)?),
                    Value::Integer(0),
                    Value::Integer(if cleared {
                        CACHE_MISS_QUALITY
                    } else {
                        NO_SLOT_QUALITY
                    }),
                    // Benilla's trailing item id: there is no item here.
                    Value::Integer(0),
                ]));
            };
            let texture =
                Value::String(lua.create_string(row.texture.as_deref().unwrap_or(MISSING_ICON))?);
            let item =
                Value::String(lua.create_string(row.name.as_deref().unwrap_or(CACHE_MISS_NAME))?);
            let quality = Value::Integer(row.quality.map_or(CACHE_MISS_QUALITY, i64::from));
            Ok(MultiValue::from_vec(vec![
                texture,
                item,
                Value::Integer(i64::from(row.quantity)),
                quality,
                // Benilla extension (5th): the item id, the shared tooltip store's key.
                Value::Integer(i64::from(row.item_id)),
            ]))
        })?,
    )?;

    // GetLootSlotLink(slot) → the row's full escaped `|cff…|Hitem:…|h[Name]|h|r` link | nil. 1-based
    // like GetLootSlotInfo beside it; nil out of range, nil for the coin row, and nil while the
    // item-template query is in flight (the link embeds the name). The reference's row click reads
    // it for BOTH modifier arms — `DressUpItemLink(GetLootSlotLink(this.slot))` (`LootFrame.lua:149`)
    // and `ChatFrameEditBox:Insert(GetLootSlotLink(this.slot))` (`:152`). Since 1751 put the stock
    // file on the chain both of those are the live call sites — our own `BenillaChatEdit_InsertLink`
    // detour is not on this path any more — and both survive the nil on their own.
    g.set(
        "GetLootSlotLink",
        lua.create_function(|lua, slot: usize| {
            let link = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model
                    .loot
                    .as_ref()
                    .and_then(|l| slot.checked_sub(1).and_then(|n| l.rows.get(n)))
                    .and_then(|r| r.as_ref()?.link.clone())
            };
            match link {
                Some(link) => Ok(Value::String(lua.create_string(&link)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // LootSlotIsItem(slot) → true for a real item row (in range, not the coin pile, not cleared).
    g.set(
        "LootSlotIsItem",
        lua.create_function(|lua, slot: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(slot
                .checked_sub(1)
                .and_then(|n| model.loot.as_ref()?.rows.get(n)?.as_ref())
                .is_some_and(|r| !r.is_coin))
        })?,
    )?;

    // LootSlotIsCoin(slot) → true for the synthesized coin pile row (false once it's been looted —
    // the cleared slot stays in the count but answers neither predicate, `LootFrame.lua:80`).
    g.set(
        "LootSlotIsCoin",
        lua.create_function(|lua, slot: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(slot
                .checked_sub(1)
                .and_then(|n| model.loot.as_ref()?.rows.get(n)?.as_ref())
                .is_some_and(|r| r.is_coin))
        })?,
    )?;

    // IsFishingLoot() → 1/nil, whether the open loot came from fishing (nil when none is open).
    // `LootFrame_OnShow` keys the reel-in sound + the fishing portrait overlay on it
    // (`LootFrame.lua:137-140`).
    g.set(
        "IsFishingLoot",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(model.loot.as_ref().is_some_and(|l| l.fishing)))
        })?,
    )?;

    // ── `LootButton`'s own method table — ONE entry, and this is it ─────────────────────────────
    //
    // `SetSlot(index)` (`0x4c1880`, table `0x847ce4`, the registrar's own `mov edx,1` says count
    // 1). 1-based in, **0-based stored** (`ftol` then `dec eax` into `[this+0x4dc]`), and it
    // pushes nothing. `LootFrame.lua:94`'s `button:SetSlot(slot)` is its only caller in the
    // shipped UI — the row's `id="N"` does NOT feed it.
    //
    // A non-number raises the client's own usage string. That is worth transcribing exactly: it
    // is the one place this class talks to a caller, and `LootFrame.lua` would surface a typo'd
    // call as this text.
    {
        let m = lua.create_table()?;
        m.set(
            "SetSlot",
            lua.create_function(|lua, (this, index): (Table, Value)| {
                let n = match &index {
                    Value::Integer(i) => *i as f64,
                    Value::Number(n) => *n,
                    // `lua_isnumber` accepts a numeric string, as everywhere else in this image.
                    Value::String(s) => match s.to_str().ok().and_then(|s| s.parse::<f64>().ok()) {
                        Some(n) => n,
                        None => return Err(mlua::Error::runtime("Usage: SetSlot(index)")),
                    },
                    _ => return Err(mlua::Error::runtime("Usage: SetSlot(index)")),
                };
                // `ftol` truncates toward zero, then `dec`. A slot below 1 leaves the row taking
                // nothing rather than wrapping — the reference stores the decrement raw, but its
                // consumer is a bounds-checked table walk and ours is an Option.
                let slot = (n.trunc() >= 1.0).then(|| n.trunc() as u32 - 1);
                super::button::set_loot_slot(lua, &this, slot)?;
                Ok(())
            })?,
        )?;
        lua.set_named_registry_value(REG_LOOTBUTTON_METHODS, m)?;
    }

    // BenillaTakeLootSlot(slot) — the ROW CLICK's take, queued as a 1-based display row; the app
    // maps it to the coin or the item's wire slot and applies the bind-on-pickup gate.
    //
    // Why this is not `LootSlot`, and why the name is ours: in 1.12 the plain take
    // is **not a Lua binding at all**. The dispatcher `0x4c2790(slot, flag)` has exactly two
    // callers — the C `CLootButton::OnClick 0x4c1820` with `flag = 0`, and the `LootSlot` binding
    // `0x4c2e70` with `flag = 1` — and only the `flag = 0` arm reaches the take. benilla has no
    // `CLootButton` widget type (our rows are ordinary XML buttons), so the click arm needs a verb,
    // and giving it the 1.12 NAME would have handed `LootSlot` a second meaning the reference does
    // not give it. `Benilla`-prefixed like the other seams the reference kept in C.
    g.set(
        "BenillaTakeLootSlot",
        lua.create_function(|lua, slot: u32| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.loot_picks.push(slot);
            Ok(())
        })?,
    )?;

    // LootSlot(slot) — the LOOT_BIND **confirmation continuation**, and nothing else, exactly as
    // 1.12 has it (`0x4c2e70`: `luaL_checknumber(1)`, `dec eax`, `0x4c2790(slot, flag = 1)`; the
    // flag-1 arm at `0x4c27c0` starts `cmp edi, [0x847cec]` and returns unless the slot IS the
    // pending confirm). So an addon calling `LootSlot(n)` on an ordinary row does nothing here,
    // which is what it does on the real client; the app owns the pending-slot gate.
    g.set(
        "LootSlot",
        lua.create_function(|lua, slot: u32| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.loot_confirms.push(slot);
            Ok(())
        })?,
    )?;

    // CloseLoot([failed]) — flag the release intent. The optional arg (the client's "couldn't open
    // the UI" signal, `LootFrame.lua:18`) is accepted and ignored; the app decides whether to send a
    // release (only when a loot is actually open).
    g.set(
        "CloseLoot",
        lua.create_function(|lua, _args: MultiValue| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.loot_close = true;
            Ok(())
        })?,
    )?;

    // GetMasterLootCandidate(index) → the 1-based candidate's NAME, or nil past the end.
    //
    // `GroupLootDropDown_Initialize` probes this sparsely and relies entirely on the nil:
    // the party arm walks `1..MAX_PARTY_MEMBERS+1` and the raid arm walks `1..40` in blocks of
    // five, keeping a "Group N" submenu only where the block's first probe answered non-nil
    // (`LootFrame.lua:169-232`). So an out-of-range index must answer nil rather than raise, and
    // the list must be DENSE — a hole would silently drop a candidate from the menu.
    g.set(
        "GetMasterLootCandidate",
        lua.create_function(|lua, index: u32| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let name = index.checked_sub(1).and_then(|i| {
                model
                    .loot
                    .as_ref()?
                    .master_candidates
                    .get(i as usize)?
                    .clone()
            });
            Ok(name)
        })?,
    )?;

    // GiveMasterLoot(slot, candidateIndex) — queue the assignment; the app owns the wire slot and
    // the recipient guid. Called from the dropdown's own handler for a below-threshold item and
    // from the CONFIRM_LOOT_DISTRIBUTION popup's OnAccept above it (`LootFrame.lua:236-244`,
    // `StaticPopup.lua:85-94`).
    g.set(
        "GiveMasterLoot",
        lua.create_function(|lua, (slot, candidate): (u32, u32)| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.loot_master_gives.push((slot, candidate));
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{LootRow, LootState};
    use crate::script::UiScript;

    fn loot() -> LootState {
        LootState {
            master_candidates: Vec::new(),
            rows: vec![
                // The coin pile, always first. No link: there is no item to link.
                Some(LootRow {
                    item_id: 0,
                    name: Some("1g 23s 45c".into()),
                    texture: Some("Interface\\Icons\\INV_Misc_Coin_01".into()),
                    quantity: 1,
                    quality: Some(1),
                    is_coin: true,
                    link: None,
                    random_property_id: 0,
                }),
                // A resolved item — name, quality AND link all landed together (one template answer).
                Some(LootRow {
                    item_id: 0,
                    name: Some("Wool Cloth".into()),
                    texture: Some("Interface\\Icons\\INV_Fabric_Wool_01".into()),
                    quantity: 3,
                    quality: Some(1),
                    is_coin: false,
                    link: Some("|cffffffff|Hitem:2589:0:0:0|h[Wool Cloth]|h|r".into()),
                    random_property_id: 0,
                }),
                // An in-flight item: the loot arrived, the item-template answer hasn't.
                Some(LootRow {
                    item_id: 0,
                    name: None,
                    texture: Some("Interface\\Icons\\INV_Misc_QuestionMark".into()),
                    quantity: 1,
                    quality: None,
                    is_coin: false,
                    link: None,
                    random_property_id: 0,
                }),
            ],
            fishing: false,
        }
    }

    #[test]
    fn loot_snapshot_reads() {
        let mut s = UiScript::new().unwrap();
        // No loot open: count 0, info nil.
        assert_eq!(s.eval::<i64>("return GetNumLootItems()").unwrap(), 0);
        assert!(s.eval::<bool>("return GetLootSlotInfo(1) == nil").unwrap());

        s.set_loot(Some(loot()));
        assert_eq!(s.eval::<i64>("return GetNumLootItems()").unwrap(), 3);

        // Row 1: the coin pile — IsCoin true, IsItem false, its text the money amount.
        assert!(s.eval::<bool>("return LootSlotIsCoin(1)").unwrap());
        assert!(s.eval::<bool>("return not LootSlotIsItem(1)").unwrap());
        let (texture, item, quantity, quality) = s
            .eval::<(String, String, i64, i64)>("return GetLootSlotInfo(1)")
            .unwrap();
        assert_eq!(texture, "Interface\\Icons\\INV_Misc_Coin_01");
        assert_eq!(item, "1g 23s 45c");
        assert_eq!((quantity, quality), (1, 1));

        // Row 2: a resolved item — IsItem true, IsCoin false, quantity + quality present.
        assert!(s.eval::<bool>("return LootSlotIsItem(2)").unwrap());
        assert!(s.eval::<bool>("return not LootSlotIsCoin(2)").unwrap());
        let (name, qty) = s
            .eval::<(String, i64)>("local _, i, q = GetLootSlotInfo(2)\nreturn i, q")
            .unwrap();
        assert_eq!((name.as_str(), qty), ("Wool Cloth", 3));

        // Row 3: in flight — the reference's cache-miss SENTINELS, not nils. The
        // quality is the load-bearing one: stock `LootFrame_Update` does
        // `ITEM_QUALITY_COLORS[quality].r` with no guard, and `-1` is a real row of that table
        // (`UIParent.lua` builds it `for i = -1, 6`) while a nil is a runtime error.
        let (texture, item, quantity, quality) = s
            .eval::<(String, String, i64, i64)>("return GetLootSlotInfo(3)")
            .unwrap();
        assert_eq!(
            (item.as_str(), quantity, quality),
            ("", 1, -1),
            "an in-flight row answers \"\" / -1, never nil"
        );
        assert_eq!(texture, "Interface\\Icons\\INV_Misc_QuestionMark");
        // …and the same row with NO display-info icon either still answers a texture path.
        let mut iconless = loot();
        iconless.rows[2].as_mut().unwrap().texture = None;
        s.set_loot(Some(iconless));
        assert_eq!(
            s.eval::<String>("return (GetLootSlotInfo(3))").unwrap(),
            "Interface\\Icons\\INV_Misc_QuestionMark",
            "a display-info miss answers the reference's question mark, not nil"
        );
        s.set_loot(Some(loot()));

        // GetLootSlotLink: the resolved item's link, nil for the coin row and for the in-flight one
        // (both arms of the reference's row click hand this straight on).
        assert_eq!(
            s.eval::<String>("return GetLootSlotLink(2)").unwrap(),
            "|cffffffff|Hitem:2589:0:0:0|h[Wool Cloth]|h|r"
        );
        assert!(s.eval::<bool>("return GetLootSlotLink(1) == nil").unwrap());
        assert!(s.eval::<bool>("return GetLootSlotLink(3) == nil").unwrap());

        // IsFishingLoot: false on an ordinary loot, true when the snapshot says fishing, false
        // again with no loot open.
        assert!(s.eval::<bool>("return not IsFishingLoot()").unwrap());
        let mut fished = loot();
        fished.fishing = true;
        s.set_loot(Some(fished));
        assert!(s.eval::<bool>("return IsFishingLoot()").unwrap());
        s.set_loot(Some(loot()));

        // Out of range → nil.
        assert!(s.eval::<bool>("return GetLootSlotInfo(9) == nil").unwrap());
        assert!(s.eval::<bool>("return GetLootSlotLink(9) == nil").unwrap());
        assert!(s.eval::<bool>("return not LootSlotIsItem(9)").unwrap());
        assert!(s.eval::<bool>("return not LootSlotIsCoin(9)").unwrap());

        // A CLEARED slot (looted): still counted — the slot list is fixed while the window is open,
        // the reference's own shape (`LootFrame.numLootItems` is read once at OnShow) — but it
        // answers neither predicate and nil info/link, so the frame hides that button in place
        // instead of collapsing the rows below it.
        let mut cleared = loot();
        cleared.rows[0] = None;
        s.set_loot(Some(cleared));
        assert_eq!(s.eval::<i64>("return GetNumLootItems()").unwrap(), 3);
        assert!(s.eval::<bool>("return GetLootSlotInfo(1) == nil").unwrap());
        assert!(s.eval::<bool>("return GetLootSlotLink(1) == nil").unwrap());
        assert!(s.eval::<bool>("return not LootSlotIsItem(1)").unwrap());
        assert!(s.eval::<bool>("return not LootSlotIsCoin(1)").unwrap());
        // …and the rows below keep their own slots.
        assert!(s.eval::<bool>("return LootSlotIsItem(2)").unwrap());
    }

    /// The master-loot half of the seam. `GetMasterLootCandidate` must answer nil
    /// past the end rather than raise — `GroupLootDropDown_Initialize` probes it sparsely (the
    /// raid arm walks 1..40 in blocks of five) and reads the nil as "nobody here".
    #[test]
    fn master_loot_candidates_read_1_based_and_nil_past_the_end() {
        let mut s = UiScript::new().unwrap();

        // No loot open at all: nil, not an error.
        assert!(s
            .eval::<bool>("return GetMasterLootCandidate(1) == nil")
            .unwrap());

        // A window with no candidate list (any loot method but master): still nil everywhere.
        s.set_loot(Some(loot()));
        assert!(s
            .eval::<bool>("return GetMasterLootCandidate(1) == nil")
            .unwrap());

        let mut ml = loot();
        ml.master_candidates = vec![Some("Thrall".into()), Some("Cairne".into())];
        s.set_loot(Some(ml));
        assert_eq!(
            s.eval::<String>("return GetMasterLootCandidate(1)")
                .unwrap(),
            "Thrall"
        );
        assert_eq!(
            s.eval::<String>("return GetMasterLootCandidate(2)")
                .unwrap(),
            "Cairne"
        );
        assert!(
            s.eval::<bool>("return GetMasterLootCandidate(3) == nil")
                .unwrap(),
            "past the end is nil"
        );
        assert!(
            s.eval::<bool>("return GetMasterLootCandidate(0) == nil")
                .unwrap(),
            "0 is not a Lua index"
        );
        // The raid arm's real sweep shape: 40 probes must all answer without raising.
        assert_eq!(
            s.eval::<i64>(
                "local n = 0
                 for i = 1, 40 do if GetMasterLootCandidate(i) then n = n + 1 end end
                 return n",
            )
            .unwrap(),
            2
        );

        // A HOLE in the middle answers nil without ending the list — the raid layout, where a
        // candidate's index carries which subgroup they are in, so slot 6 can be occupied while
        // slots 2-5 are empty. A list that stopped at the first nil would lose them.
        let mut raid = loot();
        raid.master_candidates = vec![
            Some("Thrall".into()),
            None,
            None,
            None,
            None,
            Some("Cairne".into()),
        ];
        s.set_loot(Some(raid));
        assert_eq!(
            s.eval::<String>("return GetMasterLootCandidate(1)")
                .unwrap(),
            "Thrall"
        );
        for hole in 2..=5 {
            assert!(
                s.eval::<bool>(&format!("return GetMasterLootCandidate({hole}) == nil"))
                    .unwrap(),
                "slot {hole} is a hole"
            );
        }
        assert_eq!(
            s.eval::<String>("return GetMasterLootCandidate(6)")
                .unwrap(),
            "Cairne",
            "an occupant past the holes is still reachable"
        );
    }

    #[test]
    fn give_master_loot_queues_the_assignment() {
        let mut s = UiScript::new().unwrap();
        let mut ml = loot();
        ml.master_candidates = vec![Some("Thrall".into())];
        s.set_loot(Some(ml));
        assert!(s.take_loot_master_gives().is_empty());
        s.run("GiveMasterLoot(2, 1)").unwrap();
        assert_eq!(s.take_loot_master_gives(), vec![(2, 1)]);
        assert!(s.take_loot_master_gives().is_empty(), "drained");
        // It is NOT a loot pick — the two intents must not cross wires.
        assert!(s.take_loot_picks().is_empty());
    }

    #[test]
    fn take_loot_slot_queues_picks() {
        let mut s = UiScript::new().unwrap();
        s.set_loot(Some(loot()));
        s.run("BenillaTakeLootSlot(1)").unwrap(); // coin
        s.run("BenillaTakeLootSlot(2)").unwrap(); // item
        assert_eq!(s.take_loot_picks(), vec![1, 2]);
        assert!(s.take_loot_picks().is_empty(), "drained");
    }

    /// `LootSlot` is the confirmation continuation, so it rides its OWN queue — a client that let
    /// it fall into the pick queue would loot any row an addon named, which the reference refuses.
    #[test]
    fn loot_slot_queues_confirms_not_picks() {
        let mut s = UiScript::new().unwrap();
        s.set_loot(Some(loot()));
        s.run("LootSlot(2)").unwrap();
        assert!(
            s.take_loot_picks().is_empty(),
            "LootSlot is not the take verb"
        );
        assert_eq!(s.take_loot_confirms(), vec![2]);
        assert!(s.take_loot_confirms().is_empty(), "drained");
    }

    #[test]
    fn close_loot_flags_the_intent() {
        let mut s = UiScript::new().unwrap();
        s.set_loot(Some(loot()));
        assert!(!s.take_loot_close());
        s.run("CloseLoot()").unwrap();
        assert!(s.take_loot_close());
        assert!(!s.take_loot_close(), "drained");
        // The client's failed-open form (CloseLoot(1)) is accepted and flags the same intent.
        s.run("CloseLoot(1)").unwrap();
        assert!(s.take_loot_close());
    }

    #[test]
    fn clearing_the_loot_empties_it() {
        let mut s = UiScript::new().unwrap();
        s.set_loot(Some(loot()));
        s.set_loot(None);
        assert_eq!(s.eval::<i64>("return GetNumLootItems()").unwrap(), 0);
        assert!(s.eval::<bool>("return GetLootSlotInfo(1) == nil").unwrap());
    }

    /// A real HARDWARE click on a named frame — through the pointer path, so `scripted` is false
    /// and the LootButton gate sees what it would see in play. Positions the frame first: the
    /// input path is a hit test, and an unpositioned frame is nowhere.
    fn hardware_click(s: &mut UiScript, name: &str, button: &str) {
        s.set_screen_size(1024.0, 768.0);
        s.run(&format!(
            "{name}:ClearAllPoints() {name}:SetPoint(\"BOTTOMLEFT\", 100, 100) \
             {name}:SetWidth(50) {name}:SetHeight(50) {name}:EnableMouse(true) {name}:Show()"
        ))
        .unwrap();
        s.resolve();
        s.mouse_button(125.0, 125.0, button, true);
        s.mouse_button(125.0, 125.0, button, false);
    }

    /// `LootButton` is a real `CreateFrame` type with its own identity — not an alias for Button.
    #[test]
    fn loot_button_is_its_own_registered_type() {
        let s = UiScript::new().unwrap();
        s.run(r#"lb = CreateFrame("LootButton", "LB1", UIParent)"#)
            .unwrap();
        assert_eq!(
            s.eval::<String>("return lb:GetObjectType()").unwrap(),
            "LootButton",
            "0x495b60 returns its own name, not \"Button\""
        );
        // `0x495af0` prepends its name to the base's three.
        for t in ["LootButton", "Button", "Frame", "Region"] {
            assert!(
                s.eval::<bool>(&format!("return lb:IsObjectType({t:?}) and true or false"))
                    .unwrap(),
                "IsObjectType({t:?})"
            );
        }
        assert!(!s
            .eval::<bool>(r#"return lb:IsObjectType("CheckButton") and true or false"#)
            .unwrap());
        // Its one method of its own, plus all of Button's through the chain.
        assert_eq!(
            s.eval::<String>("return type(lb.SetSlot)").unwrap(),
            "function"
        );
        assert_eq!(
            s.eval::<String>("return type(lb.SetText)").unwrap(),
            "function"
        );
        // …and the method is NOT on a plain Button — the chain runs derived → base only.
        s.run(r#"b = CreateFrame("Button", "PlainB", UIParent)"#)
            .unwrap();
        assert_eq!(s.eval::<String>("return type(b.SetSlot)").unwrap(), "nil");
    }

    /// An unmodified hardware click takes the row's slot. The Lua `OnClick` runs first and
    /// unconditionally, and its outcome does not gate the take.
    #[test]
    fn an_unmodified_click_runs_the_handler_and_then_takes() {
        let mut s = UiScript::new().unwrap();
        s.run(
            r#"
            lb = CreateFrame("LootButton", "LB1", UIParent)
            lb:SetSlot(3)
            ran = 0
            lb:SetScript("OnClick", function() ran = ran + 1 end)
        "#,
        )
        .unwrap();
        hardware_click(&mut s, "LB1", "LeftButton");
        assert_eq!(s.eval::<i64>("return ran").unwrap(), 1, "the handler ran");
        assert_eq!(s.take_loot_picks(), vec![3], "and then the take, 1-based");

        // The handler erroring does not eat the loot — `0x4c1833`'s result is never tested.
        s.run(r#"lb:SetScript("OnClick", function() error("boom") end)"#)
            .unwrap();
        hardware_click(&mut s, "LB1", "LeftButton");
        assert_eq!(s.take_loot_picks(), vec![3], "a broken hook still loots");
    }

    /// Right-click loots exactly like left-click: `0x4c1820` reads the button code once and
    /// forwards it, with no `cmp` against it anywhere in the body.
    #[test]
    fn right_click_loots_like_left_click() {
        let mut s = UiScript::new().unwrap();
        s.run(
            r#"
            lb = CreateFrame("LootButton", "LB1", UIParent)
            lb:SetSlot(2)
            lb:RegisterForClicks("LeftButtonUp", "RightButtonUp")
        "#,
        )
        .unwrap();
        hardware_click(&mut s, "LB1", "RightButton");
        assert_eq!(s.take_loot_picks(), vec![2]);
    }

    /// Any of the three modifiers suppresses the take — and only the take. The handler still runs,
    /// which is exactly how the shipped `LootFrameItem_OnClick` gets to own the ctrl and shift
    /// cases without the C take firing underneath it.
    #[test]
    fn any_modifier_suppresses_the_take_but_not_the_handler() {
        for (i, (shift, ctrl, alt)) in [
            (true, false, false),
            (false, true, false),
            (false, false, true),
        ]
        .into_iter()
        .enumerate()
        {
            let mut s = UiScript::new().unwrap();
            s.run(
                r#"
                lb = CreateFrame("LootButton", "LB1", UIParent)
                lb:SetSlot(1)
                ran = 0
                lb:SetScript("OnClick", function() ran = ran + 1 end)
            "#,
            )
            .unwrap();
            s.set_modifiers(shift, ctrl, alt);
            hardware_click(&mut s, "LB1", "LeftButton");
            assert_eq!(
                s.eval::<i64>("return ran").unwrap(),
                1,
                "case {i}: handler ran"
            );
            assert!(s.take_loot_picks().is_empty(), "case {i}: no take");
        }
    }

    /// **A scripted `:Click()` is a complete no-op** — it does not even run the row's `OnClick`.
    /// `0x4c182b` returns before the base call. Surprising enough that it is asserted rather than
    /// left to be rediscovered.
    #[test]
    fn a_scripted_click_does_nothing_at_all() {
        let mut s = UiScript::new().unwrap();
        s.run(
            r#"
            lb = CreateFrame("LootButton", "LB1", UIParent)
            lb:SetSlot(1)
            ran = 0
            lb:SetScript("OnClick", function() ran = ran + 1 end)
            lb:Click()
        "#,
        )
        .unwrap();
        assert_eq!(
            s.eval::<i64>("return ran").unwrap(),
            0,
            "the handler never ran"
        );
        assert!(s.take_loot_picks().is_empty(), "and nothing was taken");
        // A plain Button's Click() is unaffected — the gate is this type's alone.
        s.run(
            r#"
            b = CreateFrame("Button", "PlainB", UIParent)
            bran = 0
            b:SetScript("OnClick", function() bran = bran + 1 end)
            b:Click()
        "#,
        )
        .unwrap();
        assert_eq!(s.eval::<i64>("return bran").unwrap(), 1);
    }

    /// `SetSlot` is 1-based in and 0-based stored, takes a numeric string, and refuses `this` that
    /// is not a LootButton (the reference's own `IsA` guard at `0x4c18ee`).
    #[test]
    fn set_slot_converts_and_guards() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"lb = CreateFrame("LootButton", "LB1", UIParent)"#)
            .unwrap();

        // `ftol` truncates toward zero: 4.9 is row 4, not 5.
        s.run("lb:SetSlot(4.9)").unwrap();
        hardware_click(&mut s, "LB1", "LeftButton");
        assert_eq!(s.take_loot_picks(), vec![4]);

        // A numeric string is a number to `lua_isnumber`.
        s.run(r#"lb:SetSlot("2")"#).unwrap();
        hardware_click(&mut s, "LB1", "LeftButton");
        assert_eq!(s.take_loot_picks(), vec![2]);

        // A non-number raises the client's own usage text.
        let e = s.run("lb:SetSlot('x')").unwrap_err().to_string();
        assert!(e.contains("Usage: SetSlot(index)"), "{e}");

        // Never slotted → takes nothing, rather than silently taking row 1. `LB1` is still parked
        // under the cursor from the clicks above and, being linked first, would win the tie for the
        // point and take row 2 again — get it out of the way first.
        s.run("LB1:Hide()").unwrap();
        s.run(r#"fresh = CreateFrame("LootButton", "LB2", UIParent)"#)
            .unwrap();
        hardware_click(&mut s, "LB2", "LeftButton");
        assert!(s.take_loot_picks().is_empty());

        // And it is a LootButton method, not a Button one.
        s.run(r#"b = CreateFrame("Button", "PlainB", UIParent)"#)
            .unwrap();
        let e = s.run("LB1.SetSlot(b, 1)").unwrap_err().to_string();
        assert!(e.contains("not a LootButton"), "{e}");
    }
}
