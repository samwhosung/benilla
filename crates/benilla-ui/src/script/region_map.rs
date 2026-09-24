//! **The Region method map, reached by frames and regions alike** — the 19 names of the client's
//! `0x87c9b8` table, each ONE callable that works on a Frame, a Texture or a FontString.
//!
//! ## The fact this exists for (byte-verified in `WoW.exe`)
//!
//! 1.12.1's widget inheritance is not a Lua metatable chain: every class owns a flat
//! `{name, lua_CFunction}` `.data` table, and its lookup probes that table and, on a miss,
//! **tail-calls exactly one base class's lookup**:
//!
//! ```text
//! Region   0x87c9b8 (19)  lookup 0x7a2ea0  → TERMINAL (root)
//! ├─ Frame        0x878ec0 (68)  lookup 0x778590 → 0x7a2ea0
//! ├─ Texture      0x87c128 (22)  lookup 0x79c620 → 0x7a2ea0
//! └─ FontString   0x87c1d8 (32)  lookup 0x79ee20 → 0x7a2ea0
//! ```
//!
//! Parsing the two tables out of the image directly, **`Frame ∩ Region = ∅`**: not one of the 19
//! is re-registered by Frame, so every Frame-derived class reaches the *same* function a Texture
//! does. `WorldFrame.GetHeight` **is** `0x7a2030`, byte-for-byte the function
//! `someTexture:GetHeight()` resolves to.
//!
//! That is why this is not a curiosity. The idiom
//!
//! ```lua
//! local _Height = WorldFrame.GetHeight     -- pull the method off ANY widget, once
//! …
//! local h = _Height(someTexture)           -- and apply it to ANY other
//! ```
//!
//! is ordinary, correct 1.12 Lua. Quiver's `Api/Index.wow.lua` is built on it
//! (`_Height = WorldFrame.GetHeight`, `_Width = WorldFrame.GetWidth`), and benilla raised
//! `stale or invalid frame handle` on it, from inside the addon's `VARIABLES_LOADED` handler,
//! **before** the handler's last line got to publish `Quiver.CastPetAction` — which is the whole
//! of bug B267. One split method surface, one nil field, one dead addon.
//!
//! ## Why it is a bridge rather than a merge
//!
//! Our two implementations of each name are not redundant: a frame's size reads `layout_inputs`
//! and divides its edges by `GetEffectiveScale`, a region's reads `region_data` and does not; a
//! frame's `GetParent` can answer nil, a region's never can; a region's `GetPoint` resolves a
//! `relativeTo` that may be a sibling *region*. Those differences are correct, and they are the
//! same shape the reference has: `Region:GetHeight 0x7a2030` reads the receiver's layout
//! sub-object and calls `[vtable+0x20]`, which `CSimpleFontString` **overrides**. One name, one
//! entry point, per-kind behaviour behind it.
//!
//! So each of the 19 becomes one function that resolves the receiver *first* and then hands the
//! whole argument list to the arm that owns that kind — and that single function is written into
//! every method table the reference's chain would reach: the frame table, the region table, the
//! Texture and FontString leaf tables, and the title region's narrower copy. Identity holds the
//! way the binary's does — `WorldFrame.GetHeight == someTexture.GetHeight` is true here too.
//!
//! **Exactly the 19, and no more.** `Show`/`Hide`/`IsShown`/`IsVisible`/`SetAlpha`/`GetAlpha` look
//! like they belong and do not: Frame and Texture each register their *own* — `SetAlpha`, for
//! instance, is `0x774e90` on Frame and `0x79b580` on Texture — so `WorldFrame.Show(someTexture)`
//! fails on the real client and must keep failing here. (`SetSize` used to sit outside the 19 for
//! the opposite reason — in neither table because 1.12 has no such verb at all; decision 2142
//! removed it rather than filing it.) The map is the unit.

use std::collections::HashMap;
use std::rc::Rc;

use mlua::{FromLuaMulti, IntoLuaMulti, Lua, MultiValue, Table, Value};

use super::object::decode_id;
use super::{
    Model, REGION_MAP_METHODS, REG_FONTSTRING_METHODS, REG_FRAME_METHODS, REG_REGION_METHODS,
    REG_TEXTURE_METHODS, REG_TITLE_METHODS,
};

/// Which side of the object model a wrapper's `T[0]` id names. Ids come from one counter
/// ([`Model::next_id`]), so a region id can never be mistaken for a frame's.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Side {
    Frame,
    Region,
}

/// `Err` when the table is not a widget wrapper at all (that error names the real problem and is
/// worth keeping); `Ok(None)` when it *is* one whose widget no longer exists.
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

/// One side's implementation of one of the 19, type-erased to the variadic ABI.
///
/// **The erasure is the point**. The dispatcher used to hold each side as an
/// `mlua::Function` and reach it with `Function::call`, which re-enters Lua: every argument and
/// every return value round-trips through mlua's ref thread twice, and the whole call is wrapped
/// in a `lua_pcall`. Measured in a release build, a `MultiValue` relay costs **~420 ns** against
/// ~40 ns for a scalar one, and a bridged `GetWidth()` measured **~750 ns against ~99 ns** for an
/// unbridged frame method — a 7x tax on `SetPoint`, `GetWidth`, `ClearAllPoints`, `GetParent`, the
/// verbs every UI calls most. An `Rc<dyn Fn>` is one ordinary Rust call.
///
/// The ABI is exactly what `Lua::create_function` builds internally for a typed body — see
/// [`shared`], which is the only thing that makes one — so nothing about argument conversion,
/// arity or coercion changes by going through it.
pub(super) type Arm = Rc<dyn Fn(&Lua, MultiValue) -> mlua::Result<MultiValue>>;

/// The arms the two method-table installers register, collected for [`install`].
///
/// It lives in the VM's `app_data` rather than being threaded through four `install` signatures:
/// the installers are deep (`object::install` → `layout_methods::install`, `region::install` →
/// `region::layout::install`) and the collection is install-time scaffolding, removed by
/// [`install`] the moment it has been consumed.
#[derive(Default)]
pub(super) struct Arms(HashMap<(Side, &'static str), Arm>);

/// Register one of the [`REGION_MAP_METHODS`] into its side's method table **and** record its arm.
///
/// Every one of the 38 (19 names x 2 sides) goes through here instead of
/// `m.set(name, lua.create_function(body)?)?`, and [`install`] fails loudly if one did not — so a
/// name added to the map without both arms cannot reach an addon as a half-registered method.
/// The table entry it writes is transient: [`install`] replaces all 19 in every table with the
/// shared dispatcher. It exists so the pre-bridge tables are still complete for anything that
/// reads them in between (the leaf tables are *copied* out of the region table, mid-install).
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

/// Open the arm collection. Called once, at the top of [`super::object::install`], before either
/// method table is built; [`install`] closes it.
pub(super) fn open_arms(lua: &Lua) {
    lua.set_app_data(Arms::default());
}

/// Replace each of the 19 Region-map entries in every table the chain reaches with one shared
/// function that dispatches on the receiver.
///
/// Runs **after** `region::install` (which is what builds the region, leaf and title tables) and
/// after `install_frame_methods`; every table it names is already populated by then.
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
        // Both arms must exist. A missing one is not a thing to paper over with a one-sided shared
        // function: the reference gives every widget all 19, so a gap on either side is a hole in
        // the surface and says so here rather than at some addon's call site. (`GetNumPoints` was
        // exactly that hole on the frame side until this module went in.)
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
            // The receiver is argument 1 on every one of the 19 — a method call always passes it,
            // and a pulled-off `Api._Height(x)` call passes it as the only argument. Borrowed
            // rather than cloned: an mlua `Value::Table` clone registers a second reference in the
            // ref thread, and this is the entry point for the verbs a map-note redraw calls
            // thousands of times.
            let side = {
                let Some(Value::Table(this)) = args.iter().next() else {
                    return Err(mlua::Error::runtime(
                        "expected a frame or region as the first argument",
                    ));
                };
                side_of(lua, this)?
            };
            match side {
                // One ordinary Rust call, not a re-entry into Lua — see [`Arm`].
                Some(Side::Frame) => on_frame(lua, args),
                Some(Side::Region) => on_region(lua, args),
                // A wrapper whose widget is gone. Deliberately ONE message for both sides — at
                // this point the receiver's kind is exactly what could not be established, and
                // each side's own wording claimed it.
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
