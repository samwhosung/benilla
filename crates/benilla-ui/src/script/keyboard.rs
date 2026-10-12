//! Frame keyboard delivery. One dispatcher per [`Channel`] calls the frames in that channel's
//! bucket in [`walk_order`] until one consumes. Consumption is the existence of a script slot, not
//! what the handler does: a key-down consumes on `OnKeyDown` or `OnKeyUp` and fires only
//! `OnKeyDown`, so a frame with only `OnKeyUp` swallows key-downs and runs nothing; a char
//! consumes on `OnChar`. Membership is the frame's char or key bit (`0x76af00`), never a script:
//! the EditBox ctor, XML `enableKeyboard` and an XML `<Scripts>` handler set it, Lua `SetScript`
//! does not.
//!
//! An EditBox is a frame in the same walk, answering through its own vtable (`0x77a900`,
//! `0x77b160`): the focused box consumes, an unfocused one declines, and with no box focused an
//! `autoFocus` one takes the focus and the event at its own position. So a keyboard frame in a
//! higher stratum or level pre-empts the box, and one below it never sees the key.
//!
//! The key-up (`0x765fd0`) does not walk. It goes to the frame that consumed that key's last
//! down (`[root+code*4+0x84]`, written at `0x765fa2`), which `key_up` runs through the frame's
//! `OnKeyUp` slot (`0x76bba0`); the slot is never cleared by a delivered key-up.

use mlua::Lua;

use super::{event, Model};
use crate::widget::{FrameHandle, FrameKind};

/// The two walks the reference registers separately.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Channel {
    /// `0x765df0`: kind-0 buckets, vtable `+0x5c`, fires `OnChar` with the literal character.
    Char,
    /// `0x765f10`: kind-1 buckets, vtable `+0x60`, fires `OnKeyDown` with the key name.
    KeyDown,
}

/// The walk order (`0x765f10`): the channel's enabled, effectively-visible frames, strata TOOLTIP
/// down to WORLD, then level descending, ties oldest registration first as the inserter keeps
/// them (`0x764aa0`).
fn walk_order(model: &Model, channel: Channel) -> Vec<FrameHandle> {
    let mut candidates: Vec<(FrameHandle, u8, u16, u32)> = model
        .arena
        .iter_frames()
        .filter(|(_, f)| {
            f.effective_visible
                && match channel {
                    Channel::Char => f.char_enabled,
                    Channel::KeyDown => f.key_enabled,
                }
        })
        .map(|(h, f)| (h, f.strata as u8, f.level, f.insertion_seq))
        .collect();
    candidates.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)).then(a.3.cmp(&b.3)));
    candidates.into_iter().map(|(h, ..)| h).collect()
}

/// The gate's existence test on slots `+0x180`/`+0x188`/`+0x190`, kept in the script registry.
fn has_script(lua: &Lua, id: u32, name: &str) -> bool {
    event::has_widget_handler(lua, id, name)
}

/// Whether `h` is a `CSimpleEditBox`, which the walk asks [`super::editbox::takes_event`] about,
/// never a script slot: its ctor (`0x779ce8`) puts it in both buckets, and its vtable `0x81c910`
/// replaces the input slots without chaining to the base. So an unfocused box's `OnChar` (`+0x180`)
/// never runs and never eats a neighbour's keys.
fn is_editbox(lua: &Lua, h: FrameHandle) -> bool {
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    model
        .arena
        .frame(h)
        .is_some_and(|f| f.kind == FrameKind::EditBox)
}

/// One channel's walk, gate and fire. Returns whether a frame consumed the event, which is what
/// suppresses the keybinding; a handler's own return is discarded (`0x7026f0`).
fn walk(lua: &Lua, channel: Channel, arg: &str) -> bool {
    let order = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        walk_order(&model, channel)
    };
    for h in order {
        if is_editbox(lua, h) {
            if !super::editbox::takes_event(lua, h) {
                continue;
            }
            let consumed = match channel {
                Channel::Char => super::editbox::char_input(lua, arg),
                Channel::KeyDown => super::editbox::key_input(lua, arg),
            };
            if consumed {
                if channel == Channel::KeyDown {
                    remember(lua, arg, h);
                }
                return true;
            }
            continue;
        }
        let id = {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.frame_id(h)
        };
        match channel {
            // `0x76b760`: gates on the OnChar slot alone.
            Channel::Char => {
                if has_script(lua, id, "OnChar") {
                    fire(lua, id, "OnChar", arg);
                    return true;
                }
            }
            // `0x76b7d0`: either key slot consumes, only `OnKeyDown` fires.
            Channel::KeyDown => {
                let down = has_script(lua, id, "OnKeyDown");
                if down || has_script(lua, id, "OnKeyUp") {
                    remember(lua, arg, h);
                    if down {
                        fire(lua, id, "OnKeyDown", arg);
                    }
                    return true;
                }
            }
        }
    }
    false
}

/// The consumer of a key-down, which that key's up goes to (`0x765fa2`).
fn remember(lua: &Lua, key: &str, h: FrameHandle) {
    lua.app_data_mut::<Model>()
        .expect("model app_data")
        .key_consumers
        .insert(key.to_string(), h);
}

/// The key-up dispatcher (`0x765fd0`): the key's last consumer gets `OnKeyUp` with the key's name
/// (`0x76bba0`, gated on that slot alone), wherever it is now, shown or not; with no consumer on
/// record nothing consumes. The record is never cleared (the clear at `0x76605b` sits on the
/// no-record path), and an EditBox consumer takes no script: its override (`0x77b7c0`) only
/// grants an `autoFocus` box the free focus. Returns whether a consumer was on record, which
/// ends the up event for the rest of the bus (`0x76602d`).
pub(super) fn key_up(lua: &Lua, key: &str) -> bool {
    let Some(h) = lua
        .app_data_ref::<Model>()
        .expect("model app_data")
        .key_consumers
        .get(key)
        .copied()
    else {
        return false;
    };
    if is_editbox(lua, h) {
        super::editbox::takes_event(lua, h);
        return true;
    }
    let id = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        if model.arena.frame(h).is_none() {
            return true;
        }
        model.frame_id(h)
    };
    if has_script(lua, id, "OnKeyUp") {
        fire(lua, id, "OnKeyUp", key);
    }
    true
}

/// Fire one key handler with its one argument, a string on all three channels (`0x7026f0`);
/// errors are recorded in [`Model::errors`], never raised into the dispatch.
fn fire(lua: &Lua, id: u32, script: &str, arg: &str) {
    let val = mlua::Value::String(match lua.create_string(arg) {
        Ok(s) => s,
        Err(e) => {
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .record_lua_error(&e);
            return;
        }
    });
    if let Err(e) = event::fire_widget_handler(lua, id, script, vec![val]) {
        lua.app_data_mut::<Model>()
            .expect("model app_data")
            .record_lua_error(&e);
    }
}

/// The char channel (`0x765df0`): `arg1` is the character in UTF-8, a multi-byte codepoint whole
/// (`0x41abb0` encodes with no range guard).
pub(super) fn char_input(lua: &Lua, text: &str) -> bool {
    walk(lua, Channel::Char, text)
}

/// The key-down channel (`0x765f10`): `arg1` is the key name without modifiers, from the same
/// table as the keybinding chord (`0x76b7d0`, `0x4b66b0`); the host already speaks those names.
pub(super) fn key_input(lua: &Lua, key: &str) -> bool {
    walk(lua, Channel::KeyDown, key)
}

/// The key-down walk for the editing keys a focused box takes as a [`crate::script::EditAction`]
/// (BACKSPACE, DELETE, the arrows, HOME, END): a frame above the box consumes them, and the walk
/// stops at the box with `false` for its chord path, so no frame below it takes them.
pub(super) fn frame_key_input(lua: &Lua, key: &str) -> bool {
    let order = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        walk_order(&model, Channel::KeyDown)
    };
    for h in order {
        if is_editbox(lua, h) {
            if super::editbox::takes_event(lua, h) {
                remember(lua, key, h);
                return false; // the box's chord path owns this key
            }
            continue;
        }
        let id = {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.frame_id(h)
        };
        let down = has_script(lua, id, "OnKeyDown");
        if down || has_script(lua, id, "OnKeyUp") {
            remember(lua, key, h);
            if down {
                fire(lua, id, "OnKeyDown", key);
            }
            return true;
        }
    }
    false
}
