//! The Region method map: the 19 names of the reference's `0x87c9b8` table, each one callable
//! that works on a Frame, a Texture or a FontString.
//!
//! Each widget class owns a flat method table whose lookup, on a miss, tail-calls its base's:
//!
//! ```text
//! Region      0x87c9b8 (19)  lookup 0x7a2ea0, the root
//! Frame       0x878ec0 (68)  lookup 0x778590 → 0x7a2ea0
//! Texture     0x87c128 (22)  lookup 0x79c620 → 0x7a2ea0
//! FontString  0x87c1d8 (32)  lookup 0x79ee20 → 0x7a2ea0
//! ```
//!
//! Frame re-registers none of the 19, so `WorldFrame.GetHeight` is the function
//! `someTexture:GetHeight()` resolves to (`0x7a2030`), and addons pull a method off one widget to
//! apply to another. Each name is one function that resolves the receiver and hands the call to
//! that kind's arm, as `0x7a2030` calls a vtable slot `CSimpleFontString` overrides; it goes into
//! every table the chain reaches, so `WorldFrame.GetHeight == someTexture.GetHeight` holds.
//!
//! Exactly the 19: `Show`, `Hide`, `SetAlpha` and the like are per class (`SetAlpha` is `0x774e90`
//! on Frame, `0x79b580` on Texture), so `WorldFrame.Show(someTexture)` fails, as in the reference.

use std::collections::HashMap;
use std::rc::Rc;

use mlua::{FromLuaMulti, IntoLuaMulti, Lua, MultiValue, Table, Value};

use super::object::decode_id;
use super::{
    Model, REGION_MAP_METHODS, REG_FONTSTRING_METHODS, REG_FRAME_METHODS, REG_REGION_METHODS,
    REG_TEXTURE_METHODS, REG_TITLE_METHODS,
};

/// Which side of the object model a wrapper's `T[0]` id names; ids share one counter
/// ([`Model::next_id`]), so a region id is never a frame's.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Side {
    Frame,
    Region,
}

/// `Err` for a table that is no widget wrapper; `Ok(None)` for a wrapper whose widget is gone.
fn side_of(lua: &Lua, this: &Table) -> mlua::Result<Option<Side>> {
    let id = decode_id(this)?;
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    Ok(if model.id_to_frame.contains_key(&id) {
        Some(Side::Frame)
    } else if model.id_to_region.contains_key(&id) {
        Some(Side::Region)
    } else {
        None
    })
}

/// One side's implementation of one of the 19, erased to the variadic ABI so dispatch is a plain
/// Rust call: a `Function::call` would re-enter Lua through a `lua_pcall` on the hottest verbs.
/// Built only by [`set_shared`], it converts arguments exactly as `create_function` does.
pub(super) type Arm = Rc<dyn Fn(&Lua, MultiValue) -> mlua::Result<MultiValue>>;

/// The arms the method-table installers register, held in `app_data` from [`open_arms`] until
/// [`install`] consumes them.
#[derive(Default)]
pub(super) struct Arms(HashMap<(Side, &'static str), Arm>);

/// Register one of the [`REGION_MAP_METHODS`] into its side's table and record its arm; [`install`]
/// fails if a name lacks either arm. The entry written here keeps the tables complete until
/// [`install`] replaces it, since the leaf tables are copied from the region table meanwhile.
pub(super) fn set_shared<A, R, F>(
    lua: &Lua,
    m: &Table,
    side: Side,
    name: &'static str,
    f: F,
) -> mlua::Result<()>
where
    A: FromLuaMulti + 'static,
    R: IntoLuaMulti + 'static,
    F: Fn(&Lua, A) -> mlua::Result<R> + 'static,
{
    let arm: Arm =
        Rc::new(move |lua, args| f(lua, A::from_lua_multi(args, lua)?)?.into_lua_multi(lua));
    let entry = arm.clone();
    m.set(
        name,
        lua.create_function(move |lua, args: MultiValue| entry(lua, args))?,
    )?;
    lua.app_data_mut::<Arms>()
        .expect("Region-map arms — installed by `super::object::install`")
        .0
        .insert((side, name), arm);
    Ok(())
}

/// Open the arm collection, before either method table is built.
pub(super) fn open_arms(lua: &Lua) {
    lua.set_app_data(Arms::default());
}

/// Replace each of the 19 in every table the chain reaches with one function that dispatches on
/// the receiver. Runs after `region::install` and `install_frame_methods` have built the tables.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let frame: Table = lua.named_registry_value(REG_FRAME_METHODS)?;
    let region: Table = lua.named_registry_value(REG_REGION_METHODS)?;
    let leaves: Vec<Table> = [
        REG_TEXTURE_METHODS,
        REG_FONTSTRING_METHODS,
        REG_TITLE_METHODS,
    ]
    .into_iter()
    .map(|k| lua.named_registry_value::<Table>(k))
    .collect::<mlua::Result<_>>()?;

    let arms = lua
        .remove_app_data::<Arms>()
        .expect("Region-map arms — opened by `open_arms`");

    for name in REGION_MAP_METHODS {
        // The reference gives every widget all 19, so a missing arm fails here, not at an addon.
        let arm = |side: Side, which: &str| -> mlua::Result<Arm> {
            arms.0.get(&(side, name)).cloned().ok_or_else(|| {
                mlua::Error::runtime(format!(
                    "Region map: the {which} side never registered {name} through `set_shared`"
                ))
            })
        };
        let on_frame = arm(Side::Frame, "FRAME")?;
        let on_region = arm(Side::Region, "REGION")?;
        let shared = lua.create_function(move |lua, args: MultiValue| {
            // The receiver is argument 1, borrowed: a `Value::Table` clone would register a second
            // ref-thread reference on the hottest verbs.
            let side = {
                let Some(Value::Table(this)) = args.iter().next() else {
                    return Err(mlua::Error::runtime(
                        "expected a frame or region as the first argument",
                    ));
                };
                side_of(lua, this)?
            };
            match side {
                Some(Side::Frame) => on_frame(lua, args),
                Some(Side::Region) => on_region(lua, args),
                // A wrapper whose widget is gone: one message, since its kind is unknown.
                None => Err(mlua::Error::runtime("stale or invalid widget handle")),
            }
        })?;
        frame.set(name, shared.clone())?;
        region.set(name, shared.clone())?;
        for leaf in &leaves {
            leaf.set(name, shared.clone())?;
        }
    }
    Ok(())
}
