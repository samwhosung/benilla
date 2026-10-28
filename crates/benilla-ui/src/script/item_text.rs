//! The `ItemTextFrame` Lua surface (letters, books, plaques) over the read session the app owns.
//! The getters read the session through the lent host at the call ([`ItemTextQuery`]), as the
//! reference's read the reader's cells (`0xbc3fa0`, the page `0xbc3fc4`, the page list `0xbc3fd0`,
//! the text `0xbc3fe8`) and the open object as they run (`0x4e38f0`-`0x4e3b10`); the VM's own copy
//! ([`super::UiScript::set_item_text`]) answers only with no host to ask. The close and the page
//! turns are applied by the host inside their calls ([`ItemTextCall`]). The app signals
//! `ITEM_TEXT_BEGIN` and `ITEM_TEXT_READY` (`ItemTextFrame.lua:11`, `:38`) where the reader opens
//! and shows, and the close core `ITEM_TEXT_CLOSED` inside `CloseItemText()`, the frame's OnHide
//! (`ItemTextFrame.xml:349`).

use mlua::{Lua, Value};

use super::live::{ask, LiveQuery};
use super::{Model, RecordAsk};

/// The open read session as the VM's own copy holds it, which the getters read with no host to
/// ask.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ItemTextState {
    /// `ItemTextGetItem()`: the item's name, the window title.
    pub item: String,
    /// `ItemTextGetCreator()`: `ITEM_FIELD_CREATOR`'s name, `None` for an authorless text.
    pub creator: Option<String>,
    /// `ItemTextGetText()`: the current page's text.
    pub text: String,
    /// The 1-based page (`ItemTextGetPage()`); letters are always page 1.
    pub page: u32,
    /// Whether a next page exists (`ItemTextHasNextPage()`); always `false` for letters.
    pub has_next: bool,
    /// `ItemTextGetMaterial()`, e.g. "Stone"; `None` reads nil, which the frame shows as
    /// parchment (`ItemTextFrame.lua:19-21`).
    pub material: Option<String>,
}

/// The question: the reader as its getters read it at the call.
pub struct ItemTextQuery;

impl LiveQuery for ItemTextQuery {
    type Answer = ItemTextView;
}

/// The reader's cells and its object as the getters read them at the call.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ItemTextView {
    /// The open object's name (`0x4e391f`), `None` with no object (`0x4e3919`).
    pub item: Option<String>,
    /// The open item's creator (`0x4e3940`).
    pub creator: ItemTextCreator,
    /// The text cell (`0xbc3fe8`), which the show writes and an open empties (`0x4e338f`).
    pub text: String,
    /// The page cell plus one (`0x4e3adb`).
    pub page: u32,
    /// No single text (`0xbc3fbc`) and a page id after the shown one (`0x4e3b25`).
    pub has_next: bool,
    /// The open object's material (`0x4e39f0`), `None` reading nil.
    pub material: Option<String>,
}

/// The creator line as `ItemTextGetCreator` reads it (`0x4e3940`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ItemTextCreator {
    /// No open item, or a zero creator guid (`0x4e3980`, `0x4e3990`).
    #[default]
    Nobody,
    /// A name the player-name cache holds.
    Named(String),
    /// A guid the cache misses: nil, and the lookup's callback `0x4e32b0` registered
    /// ([`RecordAsk::ItemTextCreator`]).
    Unnamed(u64),
}

impl ItemTextView {
    /// The VM's own copy as the getters read it with no host.
    fn of(state: Option<&ItemTextState>) -> Self {
        let Some(s) = state else {
            return Self {
                page: 1,
                ..Self::default()
            };
        };
        Self {
            item: Some(s.item.clone()),
            creator: s
                .creator
                .clone()
                .map_or(ItemTextCreator::Nobody, ItemTextCreator::Named),
            text: s.text.clone(),
            page: s.page,
            has_next: s.has_next,
            material: s.material.clone(),
        }
    }
}

/// The reader now: the host's answer, else the VM's copy.
fn view_now(lua: &Lua) -> ItemTextView {
    ask(lua, &ItemTextQuery).unwrap_or_else(|| {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        ItemTextView::of(model.item_text.as_ref())
    })
}

impl super::UiScript {
    /// Set the VM's own copy of the open read session, or clear it with `None`, which the getters
    /// read only with no host lent.
    pub fn set_item_text(&mut self, state: Option<ItemTextState>) {
        self.model_mut().item_text = state;
    }

    /// Whether a harness's VM kept a `CloseItemText()` ([`Self::keep_calls`]).
    pub fn take_item_text_close(&mut self) -> bool {
        self.take_calls_like(&super::ScriptCall::ItemText(ItemTextCall::Close)) > 0
    }

    /// The `ItemTextPrevPage()`/`ItemTextNextPage()` turns a harness's VM kept (`-1`/`+1`, in
    /// click order).
    pub fn take_item_text_page_turns(&mut self) -> Vec<i32> {
        self.take_calls_where(|c| match c {
            super::ScriptCall::ItemText(ItemTextCall::Turn(delta)) => Some(*delta),
            _ => None,
        })
    }
}

/// An item text verb the host applies inside its call ([`super::ScriptCall::ItemText`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemTextCall {
    /// `ItemTextPrevPage()` (`0x4e3b50` → `0x4e3870`) as -1, `ItemTextNextPage()` (`0x4e3b60` →
    /// `0x4e38b0`) as +1: the page moved and shown (`0x4e35b0`), its record asked for when not
    /// cached.
    Turn(i32),
    /// `CloseItemText()` (`0x4e3b70`): the close core `0x4e32e0(0, 0)`.
    Close,
}

/// Register the item-text globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `ItemTextGetItem()` (`0x4e38f0`): the open object's name (`0x4e391f`), nil with no object
    // (`0x4e3919`), `lua_pushstring(NULL)` (`0x4e3928`).
    g.set(
        "ItemTextGetItem",
        lua.create_function(|lua, ()| match view_now(lua).item {
            Some(name) => Ok(Value::String(lua.create_string(&name)?)),
            None => Ok(Value::Nil),
        })?,
    )?;

    // `ItemTextGetCreator()` (`0x4e3940`): a name the cache misses reads nil and registers the
    // callback `0x4e32b0` (`0x4e39b9`).
    g.set(
        "ItemTextGetCreator",
        lua.create_function(|lua, ()| match view_now(lua).creator {
            ItemTextCreator::Named(name) => Ok(Value::String(lua.create_string(&name)?)),
            ItemTextCreator::Unnamed(guid) => {
                super::calls::ask(lua, RecordAsk::ItemTextCreator(guid))?;
                Ok(Value::Nil)
            }
            ItemTextCreator::Nobody => Ok(Value::Nil),
        })?,
    )?;

    // `ItemTextGetText()` (`0x4e3b00`): the text cell.
    g.set(
        "ItemTextGetText",
        lua.create_function(|lua, ()| Ok(view_now(lua).text))?,
    )?;

    // `ItemTextGetPage()` (`0x4e3ad0`): the page cell plus one, open or not.
    g.set(
        "ItemTextGetPage",
        lua.create_function(|lua, ()| Ok(i64::from(view_now(lua).page)))?,
    )?;

    // `ItemTextHasNextPage()` (`0x4e3b10`): 1, or nil.
    g.set(
        "ItemTextHasNextPage",
        lua.create_function(|lua, ()| {
            Ok(match view_now(lua).has_next {
                true => Value::Integer(1),
                false => Value::Nil,
            })
        })?,
    )?;

    // `ItemTextGetMaterial()` (`0x4e39f0`): the open object's `PageTextMaterial` name, nil with
    // none.
    g.set(
        "ItemTextGetMaterial",
        lua.create_function(|lua, ()| match view_now(lua).material {
            Some(m) => Ok(Value::String(lua.create_string(&m)?)),
            None => Ok(Value::Nil),
        })?,
    )?;

    g.set(
        "CloseItemText",
        lua.create_function(|lua, ()| {
            super::calls::submit(lua, super::ScriptCall::ItemText(ItemTextCall::Close))
        })?,
    )?;

    for (name, delta) in [("ItemTextPrevPage", -1i32), ("ItemTextNextPage", 1)] {
        g.set(
            name,
            lua.create_function(move |lua, ()| {
                super::calls::submit(lua, super::ScriptCall::ItemText(ItemTextCall::Turn(delta)))
            })?,
        )?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    fn state() -> ItemTextState {
        ItemTextState {
            item: "Plain Letter".into(),
            creator: Some("One".into()),
            text: "asd".into(),
            page: 1,
            has_next: false,
            material: None,
        }
    }

    #[test]
    fn getters_mirror_the_pushed_state() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        // No session: no object to name, so nil (`0x4e3919`); the frame reads these only while
        // shown.
        assert!(s.eval::<bool>("return ItemTextGetItem() == nil").unwrap());
        assert!(s
            .eval::<bool>("return ItemTextGetCreator() == nil")
            .unwrap());

        s.set_item_text(Some(state()));
        assert_eq!(
            s.eval::<String>("return ItemTextGetItem()").unwrap(),
            "Plain Letter"
        );
        assert_eq!(
            s.eval::<String>("return ItemTextGetCreator()").unwrap(),
            "One"
        );
        assert_eq!(s.eval::<String>("return ItemTextGetText()").unwrap(), "asd");
        assert_eq!(s.eval::<i64>("return ItemTextGetPage()").unwrap(), 1);
        assert!(s
            .eval::<bool>("return ItemTextHasNextPage() == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return ItemTextGetMaterial() == nil")
            .unwrap());

        // The close clears the session: `if ItemTextGetItem() then` is false again.
        s.set_item_text(None);
        assert!(s.eval::<bool>("return ItemTextGetItem() == nil").unwrap());
    }

    #[test]
    fn close_and_page_calls_reach_the_host() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.set_item_text(Some(state()));
        assert!(!s.take_item_text_close());
        s.run("CloseItemText()").unwrap();
        assert!(s.take_item_text_close());
        assert!(!s.take_item_text_close(), "taken");

        s.run("ItemTextNextPage() ItemTextPrevPage()").unwrap();
        assert_eq!(s.take_item_text_page_turns(), vec![1, -1]);
        assert!(s.take_item_text_page_turns().is_empty(), "taken");
    }
}
