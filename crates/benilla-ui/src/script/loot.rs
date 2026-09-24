//! The loot-window bindings: the app pushes the open loot ([`UiScript::set_loot`]), a coin row
//! first when there is gold, and the Lua verbs queue intents it drains. `slot` is 1-based; the app
//! maps a row to the money intent or the item's wire slot, which Lua never sees.
//!
//! `LootSlot` is only the LOOT_BIND confirmation: `0x4c2e70` passes flag 1 to the take dispatcher
//! `0x4c2790`, which then refuses every slot but the pending one. The take itself is flag 0, the
//! row click of [`crate::widget::FrameKind::LootButton`]. Master loot's dropdown opens from the
//! app, on a picked row whose wire `slot_type` is `MASTER`, as the dispatcher branches on that byte
//! before any Lua runs.

use mlua::{Lua, MultiValue, Table, Value};

use super::binding_abi::flag;
use super::Model;

/// `CLootButton`'s own Lua method table (`0x847ce4`): one entry, per the registrar's `mov edx,1`.
pub(super) const REG_LOOTBUTTON_METHODS: &str = "__benilla_lootbutton_methods";

/// The quality `GetLootSlotInfo` answers on an item-cache miss: the reference's -1 (`0x4c23a0`),
/// never nil. Stock `UIParent.lua:66` builds `ITEM_QUALITY_COLORS` from -1, and `LootFrame_Update`
/// indexes it unguarded (`LootFrame.lua:82`), so a nil would raise on a cold cache.
const CACHE_MISS_QUALITY: i64 = -1;

/// The row text on that miss: `0x5d8b00` leaves its buffer empty with no cache record
/// (`0x5d8b25`), and the binding pushes that empty string.
const CACHE_MISS_NAME: &str = "";

/// The icon for a row with an item but no display icon: `INV_Misc_QuestionMark` (`0x847fe4`,
/// `0x4c252b`). A slot with no item gets nil instead: `0x4c2460` returns NULL on its three guards
/// (`0x4c2470`, `0x4c24a6`, `0x4c24b7`), which `lua_pushstring 0x6f3890` pushes as nil.
const MISSING_ICON: &str = "Interface\\Icons\\INV_Misc_QuestionMark";

/// The quality of a slot with no item (past the end, slot 0, no window): `0x4c23a0`'s guards
/// return 0. A cleared slot takes -1 instead: its itemId is 0, which `0x55ba30` turns into a NULL
/// record (`0x55ba3d`/`0x55ba42`), the cache-miss arm at `0x4c2435`.
const NO_SLOT_QUALITY: i64 = 0;

/// One loot-window row, resolved by the app; its 1-based slot is its place in [`LootState::rows`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LootRow {
    /// Item name or the coin's money text; `None` while in flight, answered as [`CACHE_MISS_NAME`].
    pub name: Option<String>,
    /// Icon path; `None` without a display icon, answered as [`MISSING_ICON`].
    pub texture: Option<String>,
    /// Stack size looted; 1 for the coin row.
    pub quantity: u32,
    /// Quality 0..6; `None` while the template is in flight, answered as [`CACHE_MISS_QUALITY`].
    pub quality: Option<u32>,
    /// The synthesized coin pile (`LootSlotIsCoin` true, `LootSlotIsItem` false).
    pub is_coin: bool,
    /// The tooltip store's key, 0 for the coin row: `GetLootSlotInfo`'s fifth return, not 1.12's.
    pub item_id: u32,
    /// `GetLootSlotLink`'s answer; `None` for the coin row and while the template is in flight.
    pub link: Option<String>,
    /// The drop's random-suffix roll (`randomPropertyId`, 0 unrolled), resolved by the tooltip
    /// against [`super::Model::random_properties`]. A loot slot is no item object, so this is its
    /// only enchant source (`SetLootItem 0x533470` copies it from the loot record's `+0x14`).
    pub random_property_id: u32,
}

/// One open loot window, pushed whole by the app; `None` means no loot is open.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LootState {
    /// The fixed slot list. A looted slot becomes `None`: still counted by `GetNumLootItems`, read
    /// once at `OnShow` (`LootFrame.lua:132`), but neither item nor coin, so `LootFrame_Update`
    /// hides its button in place (`LootFrame.lua:80`).
    pub rows: Vec<Option<LootRow>>,
    /// `IsFishingLoot` (wire `loot_type` 3): the reel-in sound and portrait (`LootFrame.lua:137`).
    pub fishing: bool,
    /// The names `GetMasterLootCandidate(i)` answers by 1-based slot, from `SMSG_LOOT_MASTER_LIST`.
    /// Not packed: in a raid each candidate sits in their subgroup's five-slot block, which the
    /// dropdown labels "Group N" (`LootFrame.lua:197-213`). A `None` (an empty slot, or a name not
    /// yet resolved) answers nil.
    pub master_candidates: Vec<Option<String>>,
}

impl super::UiScript {
    /// Push (or clear, with `None`) the open loot.
    pub fn set_loot(&mut self, state: Option<LootState>) {
        self.model_mut().loot = state;
    }

    /// Drain the 1-based rows the row click took; the app maps each to the coin or the item's wire
    /// slot and applies the bind-on-pickup deferral.
    pub fn take_loot_picks(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().loot_picks)
    }

    /// Drain the 1-based rows `LootSlot` queued; the app honours only the one a LOOT_BIND confirm
    /// is pending for (the reference's `[0x847cec]` gate).
    pub fn take_loot_confirms(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().loot_confirms)
    }

    /// Whether `CloseLoot` was called since the last drain; the app releases an open loot.
    pub fn take_loot_close(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().loot_close)
    }

    /// Drain the `GiveMasterLoot(slot, candidateIndex)` pairs, both 1-based; the app resolves them
    /// to a wire slot and a guid for `CMSG_LOOT_MASTER_GIVE`.
    pub fn take_loot_master_gives(&mut self) -> Vec<(u32, u32)> {
        std::mem::take(&mut self.model_mut().loot_master_gives)
    }
}

/// Register the loot globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetNumLootItems(): cleared slots included, so it holds still while a window is open.
    g.set(
        "GetNumLootItems",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.loot.as_ref().map_or(0, |l| l.rows.len()) as i64)
        })?,
    )?;

    // GetLootSlotInfo(slot) → texture, item, quantity, quality (`0x4c2c60`, `LootFrame.lua:81`),
    // four on every leg (`0x4c2d07 mov eax,4`): no item is `nil, "", 0, 0`, a cleared slot
    // `nil, "", 0, -1`. The miss sentinels are a floor: the reference opens no window before every
    // template lands, and the app holds `LOOT_OPENED` back likewise.
    g.set(
        "GetLootSlotInfo",
        lua.create_function(|lua, slot: usize| {
            // `Some(Some(row))` live, `Some(None)` cleared, `None` no such slot.
            let slot_state = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model
                    .loot
                    .as_ref()
                    .and_then(|l| slot.checked_sub(1).and_then(|n| l.rows.get(n)))
                    .cloned()
            };
            let Some(Some(row)) = slot_state else {
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
                    // The trailing item id: none here.
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
                // The item id, a fifth return that is not a 1.12 value.
                Value::Integer(i64::from(row.item_id)),
            ]))
        })?,
    )?;

    // GetLootSlotLink(slot): nil out of range, for the coin row and while the template is in
    // flight. The stock row click hands it to `DressUpItemLink` (`LootFrame.lua:149`) and
    // `ChatFrameEditBox:Insert` (`:152`), both of which take a nil.
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

    g.set(
        "IsFishingLoot",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(model.loot.as_ref().is_some_and(|l| l.fishing)))
        })?,
    )?;

    // ── `LootButton`'s own method table ──────────────────────────────────────────────────────
    //
    // `SetSlot(index)` (`0x4c1880`): 1-based in, 0-based stored (`ftol`, `dec eax` into
    // `[this+0x4dc]`), no returns; a non-number raises the client's usage string. Its one stock
    // caller is `LootFrame.lua:94`; the row's `id` does not feed it.
    {
        let m = lua.create_table()?;
        m.set(
            "SetSlot",
            lua.create_function(|lua, (this, index): (Table, Value)| {
                let n = match &index {
                    Value::Integer(i) => *i as f64,
                    Value::Number(n) => *n,
                    // `lua_isnumber` accepts a numeric string.
                    Value::String(s) => match s.to_str().ok().and_then(|s| s.parse::<f64>().ok()) {
                        Some(n) => n,
                        None => return Err(mlua::Error::runtime("Usage: SetSlot(index)")),
                    },
                    _ => return Err(mlua::Error::runtime("Usage: SetSlot(index)")),
                };
                // `ftol` truncates toward zero, then `dec`; below 1 takes nothing, as the
                // reference's bounds-checked consumer of the raw decrement does.
                let slot = (n.trunc() >= 1.0).then(|| n.trunc() as u32 - 1);
                super::button::set_loot_slot(lua, &this, slot)?;
                Ok(())
            })?,
        )?;
        lua.set_named_registry_value(REG_LOOTBUTTON_METHODS, m)?;
    }

    // BenillaTakeLootSlot(slot): not a 1.12 verb. It queues the row click's take by 1-based row,
    // which has no Lua binding in the reference: only `CLootButton::OnClick 0x4c1820` reaches the
    // dispatcher `0x4c2790` with flag 0.
    g.set(
        "BenillaTakeLootSlot",
        lua.create_function(|lua, slot: u32| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.loot_picks.push(slot);
            Ok(())
        })?,
    )?;

    // LootSlot(slot): the LOOT_BIND confirmation only (`0x4c2e70`: `luaL_checknumber(1)`,
    // `dec eax`, `0x4c2790(slot, 1)`, whose arm at `0x4c27c0` returns unless the slot is the
    // pending confirm, `cmp edi, [0x847cec]`). On an ordinary row it does nothing.
    g.set(
        "LootSlot",
        lua.create_function(|lua, slot: u32| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.loot_confirms.push(slot);
            Ok(())
        })?,
    )?;

    // CloseLoot([failed]): the "unable to open the UI" argument (`LootFrame.lua:18`) is ignored.
    g.set(
        "CloseLoot",
        lua.create_function(|lua, _args: MultiValue| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.loot_close = true;
            Ok(())
        })?,
    )?;

    // GetMasterLootCandidate(index): nil, never an error, out of range or on an empty slot, since
    // `GroupLootDropDown_Initialize` probes slots 1-5 in a party and 1-40 in a raid and reads nil
    // as nobody (`LootFrame.lua:168-232`).
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

    // GiveMasterLoot(slot, candidateIndex): from the dropdown below the quality threshold, and from
    // the `CONFIRM_LOOT_DISTRIBUTION` popup above it (`LootFrame.lua:234-244`,
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
                // In flight: the loot arrived, its item template has not.
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
        assert_eq!(s.eval::<i64>("return GetNumLootItems()").unwrap(), 0);
        assert!(s.eval::<bool>("return GetLootSlotInfo(1) == nil").unwrap());

        s.set_loot(Some(loot()));
        assert_eq!(s.eval::<i64>("return GetNumLootItems()").unwrap(), 3);

        assert!(s.eval::<bool>("return LootSlotIsCoin(1)").unwrap());
        assert!(s.eval::<bool>("return not LootSlotIsItem(1)").unwrap());
        let (texture, item, quantity, quality) = s
            .eval::<(String, String, i64, i64)>("return GetLootSlotInfo(1)")
            .unwrap();
        assert_eq!(texture, "Interface\\Icons\\INV_Misc_Coin_01");
        assert_eq!(item, "1g 23s 45c");
        assert_eq!((quantity, quality), (1, 1));

        assert!(s.eval::<bool>("return LootSlotIsItem(2)").unwrap());
        assert!(s.eval::<bool>("return not LootSlotIsCoin(2)").unwrap());
        let (name, qty) = s
            .eval::<(String, i64)>("local _, i, q = GetLootSlotInfo(2)\nreturn i, q")
            .unwrap();
        assert_eq!((name.as_str(), qty), ("Wool Cloth", 3));

        // Row 3 is in flight: the cache-miss sentinels, not nils.
        let (texture, item, quantity, quality) = s
            .eval::<(String, String, i64, i64)>("return GetLootSlotInfo(3)")
            .unwrap();
        assert_eq!(
            (item.as_str(), quantity, quality),
            ("", 1, -1),
            "an in-flight row answers \"\" / -1, never nil"
        );
        assert_eq!(texture, "Interface\\Icons\\INV_Misc_QuestionMark");
        let mut iconless = loot();
        iconless.rows[2].as_mut().unwrap().texture = None;
        s.set_loot(Some(iconless));
        assert_eq!(
            s.eval::<String>("return (GetLootSlotInfo(3))").unwrap(),
            "Interface\\Icons\\INV_Misc_QuestionMark",
            "a display-info miss answers the reference's question mark, not nil"
        );
        s.set_loot(Some(loot()));

        assert_eq!(
            s.eval::<String>("return GetLootSlotLink(2)").unwrap(),
            "|cffffffff|Hitem:2589:0:0:0|h[Wool Cloth]|h|r"
        );
        assert!(s.eval::<bool>("return GetLootSlotLink(1) == nil").unwrap());
        assert!(s.eval::<bool>("return GetLootSlotLink(3) == nil").unwrap());

        assert!(s.eval::<bool>("return not IsFishingLoot()").unwrap());
        let mut fished = loot();
        fished.fishing = true;
        s.set_loot(Some(fished));
        assert!(s.eval::<bool>("return IsFishingLoot()").unwrap());
        s.set_loot(Some(loot()));

        assert!(s.eval::<bool>("return GetLootSlotInfo(9) == nil").unwrap());
        assert!(s.eval::<bool>("return GetLootSlotLink(9) == nil").unwrap());
        assert!(s.eval::<bool>("return not LootSlotIsItem(9)").unwrap());
        assert!(s.eval::<bool>("return not LootSlotIsCoin(9)").unwrap());

        // A looted slot stays counted but answers neither predicate, and the rows below keep
        // their slots.
        let mut cleared = loot();
        cleared.rows[0] = None;
        s.set_loot(Some(cleared));
        assert_eq!(s.eval::<i64>("return GetNumLootItems()").unwrap(), 3);
        assert!(s.eval::<bool>("return GetLootSlotInfo(1) == nil").unwrap());
        assert!(s.eval::<bool>("return GetLootSlotLink(1) == nil").unwrap());
        assert!(s.eval::<bool>("return not LootSlotIsItem(1)").unwrap());
        assert!(s.eval::<bool>("return not LootSlotIsCoin(1)").unwrap());
        assert!(s.eval::<bool>("return LootSlotIsItem(2)").unwrap());
    }

    #[test]
    fn master_loot_candidates_read_1_based_and_nil_past_the_end() {
        let mut s = UiScript::new().unwrap();

        assert!(s
            .eval::<bool>("return GetMasterLootCandidate(1) == nil")
            .unwrap());

        // No candidate list: any loot method but master.
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
        // The raid arm's sweep: 40 probes, none raising.
        assert_eq!(
            s.eval::<i64>(
                "local n = 0
                 for i = 1, 40 do if GetMasterLootCandidate(i) then n = n + 1 end end
                 return n",
            )
            .unwrap(),
            2
        );

        // A raid's slot carries the subgroup, so slot 6 can be filled while 2-5 are empty.
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

    /// In the pick queue `LootSlot` would loot any row an addon named, which the reference refuses.
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

    /// A hardware click on a named frame through the pointer path, which hit-tests, so the frame
    /// is positioned first.
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
        assert_eq!(
            s.eval::<String>("return type(lb.SetSlot)").unwrap(),
            "function"
        );
        assert_eq!(
            s.eval::<String>("return type(lb.SetText)").unwrap(),
            "function"
        );
        // A plain Button lacks it: the chain runs derived to base only.
        s.run(r#"b = CreateFrame("Button", "PlainB", UIParent)"#)
            .unwrap();
        assert_eq!(s.eval::<String>("return type(b.SetSlot)").unwrap(), "nil");
    }

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

        // An erroring handler still loots: `0x4c1833`'s result is never tested.
        s.run(r#"lb:SetScript("OnClick", function() error("boom") end)"#)
            .unwrap();
        hardware_click(&mut s, "LB1", "LeftButton");
        assert_eq!(s.take_loot_picks(), vec![3], "a broken hook still loots");
    }

    /// `0x4c1820` forwards the button code without ever comparing it.
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

    /// The handler still runs, so stock `LootFrameItem_OnClick` owns ctrl and shift alone.
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

    /// A scripted `:Click()` does not even run `OnClick`: `0x4c182b` returns before the base call.
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
        // A plain Button's Click() is unaffected.
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

    /// The `this` guard is the reference's `IsA` check at `0x4c18ee`.
    #[test]
    fn set_slot_converts_and_guards() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"lb = CreateFrame("LootButton", "LB1", UIParent)"#)
            .unwrap();

        // `ftol` truncates toward zero: 4.9 is row 4, not 5.
        s.run("lb:SetSlot(4.9)").unwrap();
        hardware_click(&mut s, "LB1", "LeftButton");
        assert_eq!(s.take_loot_picks(), vec![4]);

        s.run(r#"lb:SetSlot("2")"#).unwrap();
        hardware_click(&mut s, "LB1", "LeftButton");
        assert_eq!(s.take_loot_picks(), vec![2]);

        let e = s.run("lb:SetSlot('x')").unwrap_err().to_string();
        assert!(e.contains("Usage: SetSlot(index)"), "{e}");

        // Never slotted takes nothing. `LB1` still sits under the cursor and, linked first, would
        // win the hit test, so it hides first.
        s.run("LB1:Hide()").unwrap();
        s.run(r#"fresh = CreateFrame("LootButton", "LB2", UIParent)"#)
            .unwrap();
        hardware_click(&mut s, "LB2", "LeftButton");
        assert!(s.take_loot_picks().is_empty());

        s.run(r#"b = CreateFrame("Button", "PlainB", UIParent)"#)
            .unwrap();
        let e = s.run("LB1.SetSlot(b, 1)").unwrap_err().to_string();
        assert!(e.contains("not a LootButton"), "{e}");
    }
}
