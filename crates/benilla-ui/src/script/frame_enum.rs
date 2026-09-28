//! `GetNumFrames` (`0x705f00`) and `EnumerateFrames` (`0x705f60`), registered beside `CreateFrame`
//! from the frame-script table `0x872e74`: both walk the UI root's list of every live frame
//! (`root+0xcc4`), which a frame joins at construction and leaves at destruction, hidden or not.
//! Regions are not frames and are not on it. Here the list is the frame ids in ascending order,
//! since every creation path mints its frame's id at once ([`Model::frame_id`]).

use mlua::{Lua, MultiValue, Value};

use super::object::{decode_id, frame_wrapper};
use super::Model;

/// `0x872f08`, verbatim.
const NO_THIS: &str = "EnumerateFrames: Couldn't find 'this' in current object";
/// `0x872ecc`, verbatim.
const NOT_A_FRAME: &str = "EnumerateFrames: Wrong current object type, expected frame";

/// The first live frame with an id above `after`, in creation order.
fn next_frame(model: &Model, after: u32) -> Option<u32> {
    (after.saturating_add(1)..model.next_id).find(|id| {
        model
            .id_to_frame
            .get(id)
            .is_some_and(|h| model.arena.frame(*h).is_some())
    })
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `GetNumFrames()`: the list's length as one number; any arguments are ignored.
    g.set(
        "GetNumFrames",
        lua.create_function(|lua, _: MultiValue| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.arena.iter_frames().count() as f64)
        })?,
    )?;

    // `EnumerateFrames([frame])`: the frame after `frame` on the list, or the first when the
    // argument is not a table (`lua_type(L, 1) == 5`, `0x705f6e`); nil past the last. A table is
    // read as an object through its `[0]` (`lua_rawgeti`, `0x705f7c`): none raises `0x872f08`,
    // and an object that is not a frame, a Texture or FontString, raises `0x872ecc`.
    g.set(
        "EnumerateFrames",
        lua.create_function(|lua, args: MultiValue| {
            let after = match args.front() {
                Some(Value::Table(t)) => {
                    let id = match t.raw_get::<Value>(0)? {
                        Value::LightUserData(l) if !l.0.is_null() => decode_id(t)?,
                        _ => return Err(mlua::Error::runtime(NO_THIS)),
                    };
                    let model = lua.app_data_ref::<Model>().expect("model app_data");
                    if !model.id_to_frame.contains_key(&id) {
                        return Err(mlua::Error::runtime(NOT_A_FRAME));
                    }
                    id
                }
                _ => 0,
            };
            let next = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                next_frame(&model, after)
            };
            match next {
                Some(id) => Ok(Value::Table(frame_wrapper(lua, id)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    #[test]
    fn enumerate_frames_walks_every_live_frame_in_creation_order() {
        let s = UiScript::new().unwrap();
        s.run(
            "A = CreateFrame('Frame', 'EnumA') \
             A:CreateTexture('EnumATex') \
             B = CreateFrame('Button', nil, A) B:Hide() \
             C = CreateFrame('Frame', 'EnumC')",
        )
        .unwrap();
        let (count, walked, tail): (i64, i64, Vec<bool>) = s
            .eval(
                "local n, f, seen = 0, EnumerateFrames(), {} \
                 while f do n = n + 1 table.insert(seen, f) f = EnumerateFrames(f) end \
                 local k = table.getn(seen) \
                 return GetNumFrames(), n, { seen[k - 2] == A, seen[k - 1] == B, seen[k] == C }",
            )
            .unwrap();
        assert_eq!(
            walked, count,
            "the walk visits exactly GetNumFrames() frames"
        );
        assert_eq!(
            tail,
            [true, true, true],
            "the three newest frames close the walk in creation order, the hidden and nameless \
             one included, and the texture is not a frame"
        );
        assert_eq!(s.arity("GetNumFrames()").unwrap(), 1);
        assert_eq!(s.arity("EnumerateFrames(C)").unwrap(), 1);
        assert!(s.eval::<bool>("return EnumerateFrames(C) == nil").unwrap());
        // A non-table argument starts over.
        assert!(s
            .eval::<bool>("return EnumerateFrames(5) == EnumerateFrames() and EnumerateFrames('x') == EnumerateFrames()")
            .unwrap());
    }

    #[test]
    fn enumerate_frames_refuses_what_is_not_a_frame() {
        let s = UiScript::new().unwrap();
        s.run("F = CreateFrame('Frame', 'EnumF') T = F:CreateTexture('EnumT')")
            .unwrap();
        let e = s.run("EnumerateFrames({})").unwrap_err().to_string();
        assert!(
            e.contains("EnumerateFrames: Couldn't find 'this' in current object"),
            "{e}"
        );
        let e = s.run("EnumerateFrames(T)").unwrap_err().to_string();
        assert!(
            e.contains("EnumerateFrames: Wrong current object type, expected frame"),
            "{e}"
        );
    }
}
