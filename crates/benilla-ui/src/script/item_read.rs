//! What an item answers, which the item and bag getters read off the item cache and the item
//! object as the call runs. The reference's getters look the slot's object up and read its record
//! off the item cache there (`GetContainerItemInfo`'s `0x6228a0`, then `0x55ba30`; the trade
//! getters' `0x4c017f`, `0x4c0582`), so a getter the lent host can answer reads the field it
//! answers now, not the frame's snapshot ([`read_item`]): a record that has landed answers in the
//! call.

use mlua::Lua;

/// The item a getter reads, as its slot names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemKey {
    /// An item object the engine holds, by guid: its descriptor, and its record by its entry.
    Object(u64),
    /// A record alone, by entry, with the roll the slot's wire block carries (a partner's trade
    /// slot, a letter's attachment, an inspected player's visible item).
    Record { entry: u32, roll: ItemRoll },
}

/// The three numbers an item's link and full name carry beside its entry (`0x52adb0`): the
/// permanent enchant, the random property and its suffix factor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ItemRoll {
    pub enchant: u32,
    pub property: u32,
    pub suffix_factor: u32,
}

/// What an item answers, each field read on its own: off the item cache and the item object
/// through the lent host ([`crate::script::CallHost::read_item`]), or off a snapshot, which holds
/// them as its build read them. The app's one derivation of each is its implementation of this,
/// which its snapshots' builds read too.
pub trait ItemRead {
    /// The item cache holds the record.
    fn cached(&self) -> bool;
    /// The full name (`0x5d8bc0`): the record's, with the roll's suffix; `None` while uncached.
    fn name(&self) -> Option<String>;
    /// The record's display icon; `None` while uncached or for a display with none, which the
    /// getters show as the question mark.
    fn texture(&self) -> Option<String>;
    /// The record's quality; `None` while uncached.
    fn quality(&self) -> Option<u32>;
    /// The record's `InventoryType`; 0 while uncached and for an item that cannot be equipped.
    fn inventory_type(&self) -> u32;
    /// The record's class; `None` while uncached.
    fn class(&self) -> Option<u32>;
    /// The link (`0x52adb0`) once the record is cached, which embeds its name and quality.
    fn link(&self) -> Option<String>;
    /// An item object's link while its record is not cached, white and nameless (`0x52ae00`:
    /// `0x52aec5`, `0x5d8b40`); `None` once cached, and for a record read.
    fn uncached_link(&self) -> Option<String>;
    /// An item object's `ITEM_FIELD_STACK_COUNT`, 1 when unsent; `None` for a record read, whose
    /// count is its slot's.
    fn stack_count(&self) -> Option<u32>;
    /// An item object's current and maximum durability, `None` with a maximum of 0 or for a
    /// record read.
    fn durability(&self) -> Option<(u32, u32)>;
    /// The record carries PageText, or the object a letter's `ITEM_FIELD_ITEM_TEXT_ID`
    /// (`0x4f97ba`-`0x4f97cf`).
    fn readable(&self) -> bool;
    /// The record's vendor price (`[rec+0x28]`); `None` while uncached and for a snapshot that
    /// holds none.
    fn sell_price(&self) -> Option<u32> {
        None
    }
    /// `GetItemInfo`'s record fields past the name and the quality, `None` while uncached: the
    /// required level, the stack size (`+0x60`), and the `ItemClass.dbc` and `ItemSubClass.dbc`
    /// names (`0x48e070`, `0x48e311`).
    fn info(&self) -> Option<ItemInfoFields> {
        None
    }
}

/// [`ItemRead::info`]: what `GetItemInfo` answers of a record beside its name, quality, slot and
/// icon.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ItemInfoFields {
    pub required_level: u32,
    pub stackable: u32,
    pub item_type: Option<String>,
    pub item_sub_type: Option<String>,
}

/// `f` over the item `key` names, with `snapshot`, the view of it the getter's slot holds: through
/// the lent host when one is free and holds the item, the live read; else, with no host to ask (a
/// glue VM, or a call's apply under way), no key, or an object the engine no longer holds, the
/// snapshot answers both.
pub(crate) fn read_item<V: ItemRead, T>(
    lua: &Lua,
    key: Option<ItemKey>,
    snapshot: &V,
    f: impl FnOnce(&V, &dyn ItemRead) -> T,
) -> T {
    let mut f = Some(f);
    let live = read_live(lua, key, |live| f.take().map(|f| f(snapshot, live))).flatten();
    match live {
        Some(out) => out,
        None => f.take().expect("an unread `f`")(snapshot, snapshot),
    }
}

/// `f` over the item `key` names through the lent host, else over the snapshot `snapshot` hands
/// it, which borrows the snapshot only on that fall and copies only what `f` answers.
pub(crate) fn read_item_or<T>(
    lua: &Lua,
    key: Option<ItemKey>,
    f: impl Fn(&dyn ItemRead) -> T,
    snapshot: impl FnOnce(&dyn Fn(&dyn ItemRead) -> T) -> Option<T>,
) -> Option<T> {
    read_live(lua, key, &f).or_else(|| snapshot(&f))
}

/// `f` over the item `key` names through the lent host; `None` where [`read_item`] falls to the
/// snapshot.
pub(crate) fn read_live<T>(
    lua: &Lua,
    key: Option<ItemKey>,
    f: impl FnOnce(&dyn ItemRead) -> T,
) -> Option<T> {
    let key = key?;
    let mut f = Some(f);
    let mut out = None;
    super::host::read_host(lua, |h| {
        h.read_item(key, &mut |live| {
            out = f.take().map(|f| f(live));
        })
    });
    out
}

/// The VM's record copy, as `GetItemInfo` reads it off the snapshot.
impl ItemRead for super::ItemTemplateView {
    fn cached(&self) -> bool {
        true
    }
    fn name(&self) -> Option<String> {
        Some(self.name.clone())
    }
    fn texture(&self) -> Option<String> {
        self.icon.clone()
    }
    fn quality(&self) -> Option<u32> {
        Some(self.quality)
    }
    fn inventory_type(&self) -> u32 {
        self.inventory_type
    }
    fn class(&self) -> Option<u32> {
        Some(self.class)
    }
    fn link(&self) -> Option<String> {
        None
    }
    fn uncached_link(&self) -> Option<String> {
        None
    }
    fn stack_count(&self) -> Option<u32> {
        None
    }
    fn durability(&self) -> Option<(u32, u32)> {
        None
    }
    fn readable(&self) -> bool {
        false
    }
    fn sell_price(&self) -> Option<u32> {
        Some(self.sell_price)
    }
    fn info(&self) -> Option<ItemInfoFields> {
        Some(ItemInfoFields {
            required_level: self.required_level,
            stackable: self.stackable,
            item_type: self.item_type.clone(),
            item_sub_type: self.item_sub_type.clone(),
        })
    }
}

/// A bag slot's snapshot, as [`super::container`]'s getters read it; it holds no name, which none
/// of them answers.
impl ItemRead for super::ContainerSlot {
    fn cached(&self) -> bool {
        self.quality.is_some()
    }
    fn name(&self) -> Option<String> {
        None
    }
    fn texture(&self) -> Option<String> {
        self.texture.clone()
    }
    fn quality(&self) -> Option<u32> {
        self.quality
    }
    fn inventory_type(&self) -> u32 {
        self.inventory_type
    }
    fn class(&self) -> Option<u32> {
        self.item_class
    }
    fn link(&self) -> Option<String> {
        self.link.clone()
    }
    fn uncached_link(&self) -> Option<String> {
        self.uncached_link.clone()
    }
    fn stack_count(&self) -> Option<u32> {
        Some(self.count)
    }
    fn durability(&self) -> Option<(u32, u32)> {
        self.durability
    }
    fn readable(&self) -> bool {
        self.readable
    }
}

/// An inventory slot's snapshot, as the inventory getters read it; it holds no class or readable
/// flag, which none of them answers.
impl ItemRead for super::InvSlotView {
    fn cached(&self) -> bool {
        self.inventory_type.is_some()
    }
    fn name(&self) -> Option<String> {
        self.name.clone()
    }
    fn texture(&self) -> Option<String> {
        self.icon.clone()
    }
    fn quality(&self) -> Option<u32> {
        self.inventory_type.map(|_| self.quality as u32)
    }
    fn inventory_type(&self) -> u32 {
        self.inventory_type.unwrap_or(0)
    }
    fn class(&self) -> Option<u32> {
        None
    }
    fn link(&self) -> Option<String> {
        self.link.clone()
    }
    fn uncached_link(&self) -> Option<String> {
        self.uncached_link.clone()
    }
    fn stack_count(&self) -> Option<u32> {
        Some(self.count)
    }
    fn durability(&self) -> Option<(u32, u32)> {
        self.durability
    }
    fn readable(&self) -> bool {
        false
    }
}

/// A trade slot's snapshot, as the trade getters read it; it holds no class or readable flag,
/// which none of them answers.
impl ItemRead for super::TradeSlotItem {
    fn cached(&self) -> bool {
        self.name.is_some()
    }
    fn name(&self) -> Option<String> {
        self.name.clone()
    }
    fn texture(&self) -> Option<String> {
        self.texture.clone()
    }
    fn quality(&self) -> Option<u32> {
        self.quality
    }
    fn inventory_type(&self) -> u32 {
        self.inventory_type
    }
    fn class(&self) -> Option<u32> {
        None
    }
    fn link(&self) -> Option<String> {
        self.link.clone()
    }
    fn uncached_link(&self) -> Option<String> {
        self.uncached_link.clone()
    }
    fn stack_count(&self) -> Option<u32> {
        Some(self.count)
    }
    fn durability(&self) -> Option<(u32, u32)> {
        Some(self.durability)
    }
    fn readable(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{ItemKey, ItemRead};
    use crate::script::cursor::CursorItem;
    use crate::script::{
        CallHost, CallTail, ScriptCall, TradeSideState, TradeSlotItem, TradeState, UiScript,
    };

    const HELD: u64 = 0x4000_0000_0000_0102;

    /// A host that holds one item object, [`HELD`], whose stack is `stack` now.
    struct Holding {
        stack: u32,
    }

    /// [`Holding`]'s object: a cached record, its stack as the object holds it.
    struct Stack(u32);

    impl ItemRead for Stack {
        fn cached(&self) -> bool {
            true
        }
        fn name(&self) -> Option<String> {
            Some("Linen Cloth".into())
        }
        fn texture(&self) -> Option<String> {
            None
        }
        fn quality(&self) -> Option<u32> {
            Some(1)
        }
        fn inventory_type(&self) -> u32 {
            0
        }
        fn class(&self) -> Option<u32> {
            Some(7)
        }
        fn link(&self) -> Option<String> {
            None
        }
        fn uncached_link(&self) -> Option<String> {
            None
        }
        fn stack_count(&self) -> Option<u32> {
            Some(self.0)
        }
        fn durability(&self) -> Option<(u32, u32)> {
            None
        }
        fn readable(&self) -> bool {
            false
        }
    }

    impl CallHost for Holding {
        fn apply(&mut self, _: &mut UiScript, _: ScriptCall) -> Option<CallTail> {
            None
        }
        fn read_item(&mut self, key: ItemKey, read: &mut dyn FnMut(&dyn ItemRead)) -> bool {
            if key != ItemKey::Object(HELD) {
                return false;
            }
            read(&Stack(self.stack));
            true
        }
    }

    fn held(count: Option<u32>) -> CursorItem {
        CursorItem {
            guid: HELD,
            bag: 0,
            slot: 1,
            item_id: 2589,
            item_class: None,
            texture: None,
            link: None,
            count,
            quality: Some(1),
            equip_slots: Vec::new(),
            bar_placeable: true,
            consumable: false,
        }
    }

    /// Our own trade slot's count is the object's stack as the call runs (`0x4c055d`-`0x4c056e`,
    /// `[item+0x114]+0x20`), not the push's.
    #[test]
    fn our_trade_slots_count_reads_the_objects_stack() {
        let mut s = UiScript::new().unwrap();
        let mut player = TradeSideState::default();
        player.slots[0] = Some(TradeSlotItem {
            item_id: 2589,
            count: 2,
            name: Some("Linen Cloth".into()),
            ..Default::default()
        });
        s.set_trade(Some(TradeState {
            player,
            target: TradeSideState::default(),
        }));
        s.model_mut().trade_offered[0] = Some(held(None));
        let chunk = "local _, _, count = GetTradePlayerItemInfo(1) return count";
        assert_eq!(s.eval::<i64>(chunk).unwrap(), 2, "off the push");
        let live = s.hosted(&mut Holding { stack: 5 }, |s| s.eval::<i64>(chunk).unwrap());
        assert_eq!(live, 5);
    }

    /// The letter's attachment count is the object's stack as the call runs (`0x4ae68e`).
    #[test]
    fn the_send_mail_items_count_reads_the_objects_stack() {
        let mut s = UiScript::new().unwrap();
        s.set_item_template(
            2589,
            crate::script::ItemTemplateView {
                name: "Linen Cloth".into(),
                quality: 1,
                ..Default::default()
            },
        );
        s.model_mut().mail_send_item = Some(held(Some(3)));
        let chunk = "local _, _, count = GetSendMailItem() return count";
        assert_eq!(s.eval::<i64>(chunk).unwrap(), 3, "off the cursor");
        let live = s.hosted(&mut Holding { stack: 7 }, |s| s.eval::<i64>(chunk).unwrap());
        assert_eq!(live, 7);
    }
}
