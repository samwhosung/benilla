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
//! down (`[root+code*4+0x84]`, written at `0x765fa2`), through its `OnKeyUp` slot (`0x76bba0`),
//! and the auto-repeat (`0x766070`, category `0xa`) to the same frame's `+0x58`: a stub on a plain
//! frame (`0x46ff90`), the key-down again on an EditBox (`0x77b770`). A delivered up never clears
//! the record.

use mlua::Lua;

use super::{event, Model};
use crate::widget::{FrameHandle, FrameKind};

/// What the key-down walk (`0x765f10`) came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyWalk {
    /// No frame took the key: the world frame's binding is next.
    Declined,
    /// A frame took it (`0x76b7d0`'s existence gate): nothing below it, the focused box and the
    /// keybinding included, sees it.
    Frame,
    /// The walk reached an EditBox that took it: focused already or, with none focused, an
    /// `autoFocus` box that just took the focus (`0x77b1c7`-`0x77b1db`). The key's editing action
    /// is that box's.
    Box,
}

/// The two walks the reference registers separately.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Channel {
    /// `0x765df0`: kind-0 buckets, vtable `+0x5c`, fires `OnChar` with the literal character.
    Char,
    /// `0x765f10`: kind-1 buckets, vtable `+0x60`, fires `OnKeyDown` with the key name.
    KeyDown,
}

/// The walk order (`0x765f10`): the channel's enabled, effectively-visible frames, strata TOOLTIP
/// down to WORLD, then level descending, ties oldest first by when each joined the bucket
/// (`0x764aa0` inserts after the equal levels; a show, a level or strata change and a live enable
/// each insert again).
fn walk_order(model: &Model, channel: Channel) -> Vec<FrameHandle> {
    let mut candidates: Vec<(FrameHandle, u8, u16, u64)> = model
        .arena
        .iter_frames()
        .filter(|(_, f)| {
            f.effective_visible
                && match channel {
                    Channel::Char => f.char_enabled,
                    Channel::KeyDown => f.key_enabled,
                }
        })
        .map(|(h, f)| {
            let stamp = match channel {
                Channel::Char => f.char_stamp,
                Channel::KeyDown => f.key_stamp,
            };
            (h, f.strata as u8, f.level, stamp)
        })
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

/// Whether the EditBox `h` declines `key` before it looks at focus (`0x77b160`): PRINTSCREEN
/// (`0x77b17f`), and the four arrows on a box in alt-arrow mode (`ignoreArrows`) with ALT up
/// (`0x77b18e`-`0x77b1c4`), so the walk goes on below it and the world frame's binding can run.
fn box_declines(lua: &Lua, h: FrameHandle, key: &str) -> bool {
    if key.eq_ignore_ascii_case("PRINTSCREEN") {
        return true;
    }
    let arrow = ["LEFT", "UP", "RIGHT", "DOWN"]
        .iter()
        .any(|a| key.eq_ignore_ascii_case(a));
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    arrow
        && !model.alt_down
        && model.arena.frame(h).is_some_and(|f| {
            matches!(&f.kind_state,
                crate::widget::KindState::EditBox(eb) if eb.alt_arrow_key_mode)
        })
}

/// Whether the frame's own key or char slot takes the event, firing it: the base gates
/// `0x76b760` (`OnChar` alone) and `0x76b7d0` (either key slot, `OnKeyDown` fired).
fn frame_takes(lua: &Lua, h: FrameHandle, channel: Channel, arg: &str) -> bool {
    let id = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        model.frame_id(h)
    };
    match channel {
        Channel::Char => {
            if has_script(lua, id, "OnChar") {
                fire(lua, id, "OnChar", arg);
                return true;
            }
        }
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
    false
}

/// The char walk (`0x765df0`): whether a frame or a box took the character.
fn walk_char(lua: &Lua, text: &str) -> bool {
    let order = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        walk_order(&model, Channel::Char)
    };
    for h in order {
        if is_editbox(lua, h) {
            if super::editbox::takes_event(lua, h) && super::editbox::char_input(lua, text) {
                return true;
            }
        } else if frame_takes(lua, h, Channel::Char, text) {
            return true;
        }
    }
    false
}

/// The key-down walk (`0x765f10`). A handler's own return is discarded (`0x7026f0`).
fn walk_key(lua: &Lua, key: &str) -> KeyWalk {
    let order = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        walk_order(&model, Channel::KeyDown)
    };
    for h in order {
        if is_editbox(lua, h) {
            if box_declines(lua, h, key) || !super::editbox::takes_event(lua, h) {
                continue;
            }
            if super::editbox::key_input(lua, key) {
                remember(lua, key, h);
                return KeyWalk::Box;
            }
        } else if frame_takes(lua, h, Channel::KeyDown, key) {
            return KeyWalk::Frame;
        }
    }
    KeyWalk::Declined
}

/// The XML walker's auto-enable after a `SetScript` (`0x769ef0`): `OnChar` sets the char bit
/// (`0x769f6b`), `OnKeyDown` and `OnKeyUp` the key bit (`0x769fa4`), each alone. Lua's `SetScript`
/// never does (`0x7748d0`).
pub(crate) fn enable_for_script(lua: &Lua, this: &mlua::Table, script: &str) -> mlua::Result<()> {
    let char_bit = script.eq_ignore_ascii_case("OnChar");
    let key_bit =
        script.eq_ignore_ascii_case("OnKeyDown") || script.eq_ignore_ascii_case("OnKeyUp");
    if !(char_bit || key_bit) {
        return Ok(());
    }
    let h = super::object::frame_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    if char_bit {
        model.arena.set_char_enabled(h, true);
    } else {
        model.arena.set_key_enabled(h, true);
    }
    Ok(())
}

/// The consumer of a key-down, which that key's up and repeats go to (`0x765fa2`).
fn remember(lua: &Lua, key: &str, h: FrameHandle) {
    lua.app_data_mut::<Model>()
        .expect("model app_data")
        .key_consumers
        .insert(key.to_string(), h);
}

/// A binding took the key's press: the world frame's key-down (`0x483b70`) ran it and returned
/// that, so the world frame is the consumer, and the up and repeats go to it
/// (`0x483bd0`), not to whichever frame took the key before.
pub(super) fn note_world_key(lua: &Lua, key: &str) {
    let world = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        let found = model
            .arena
            .iter_frames()
            .find(|(_, f)| f.kind == FrameKind::WorldFrame)
            .map(|(h, _)| h);
        found
    };
    if let Some(h) = world {
        remember(lua, key, h);
    }
}

/// Whether `h` is linked, the base handlers' first test (`[+0xd4]`, `0x76bba7`, `0x77b7c0`).
fn linked(lua: &Lua, h: FrameHandle) -> bool {
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    model.arena.frame(h).is_some_and(|f| f.effective_visible)
}

/// What the key-up dispatcher (`0x765fd0`) did with a release.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyUp {
    /// A consumer was on record and got the up (or was gone or hidden, which ends it the same).
    Delivered,
    /// No consumer on record, and the release was PRINTSCREEN, which the client runs through the
    /// key-down walk (`0x76603c`-`0x76604d`): Windows sends that key as an up alone. `Declined`
    /// leaves the world frame's binding to the host.
    Walked(KeyWalk),
    /// No consumer on record: nothing.
    Unclaimed,
}

impl KeyUp {
    /// Whether the release was taken, by a recorded consumer or the PRINTSCREEN walk.
    pub fn taken(self) -> bool {
        !matches!(self, KeyUp::Unclaimed | KeyUp::Walked(KeyWalk::Declined))
    }
}

/// The key-up dispatcher (`0x765fd0`): the key's last consumer gets `OnKeyUp` with the key's name
/// (`0x76bba0`, gated on that slot alone, and on being linked), however the key stands now. An
/// EditBox consumer runs no script: its override (`0x77b7c0`) only grants an `autoFocus` box the
/// free focus. A delivered up leaves the record. With none on record the release is nothing,
/// except PRINTSCREEN's, which runs the key-down walk and then clears the record that walk wrote
/// (`0x76603c`-`0x76605b`).
pub(super) fn key_up(lua: &Lua, key: &str) -> KeyUp {
    let Some(h) = lua
        .app_data_ref::<Model>()
        .expect("model app_data")
        .key_consumers
        .get(key)
        .copied()
    else {
        if !key.eq_ignore_ascii_case("PRINTSCREEN") {
            return KeyUp::Unclaimed;
        }
        let walked = walk_key(lua, key);
        lua.app_data_mut::<Model>()
            .expect("model app_data")
            .key_consumers
            .remove(key);
        return KeyUp::Walked(walked);
    };
    if !linked(lua, h) {
        return KeyUp::Delivered;
    }
    if is_editbox(lua, h) {
        super::editbox::takes_event(lua, h);
        return KeyUp::Delivered;
    }
    let id = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        model.frame_id(h)
    };
    if has_script(lua, id, "OnKeyUp") {
        fire(lua, id, "OnKeyUp", key);
    }
    KeyUp::Delivered
}

/// The auto-repeat dispatcher (`0x766070`): the key's consumer's `+0x58`, and any consumer on
/// record makes the repeat consumed whatever that returns (`0x7660bb`). A plain frame's is a stub
/// (`0x46ff90`): [`KeyWalk::Frame`], nothing runs. An EditBox's (`0x77b770`) grants an `autoFocus`
/// box the free focus first (`0x77b799`) and then runs its key-down, which may decline: the
/// repeat is [`KeyWalk::Box`] only when the box acted, else `Frame`, consumed and inert. No
/// consumer on record: `Declined`, and nothing happens.
pub(super) fn key_repeat(lua: &Lua, key: &str) -> KeyWalk {
    let Some(h) = lua
        .app_data_ref::<Model>()
        .expect("model app_data")
        .key_consumers
        .get(key)
        .copied()
    else {
        return KeyWalk::Declined;
    };
    if is_editbox(lua, h)
        && linked(lua, h)
        && super::editbox::takes_event(lua, h)
        && !box_declines(lua, h, key)
        && super::editbox::key_input(lua, key)
    {
        return KeyWalk::Box;
    }
    KeyWalk::Frame
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
    walk_char(lua, text)
}

/// The key-down channel (`0x765f10`): `arg1` is the key name without modifiers, from the same
/// table as the keybinding chord (`0x76b7d0`, `0x4b66b0`); the host already speaks those names.
pub(super) fn key_down(lua: &Lua, key: &str) -> KeyWalk {
    walk_key(lua, key)
}
