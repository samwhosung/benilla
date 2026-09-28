//! The Video options getter/setter pairs beside `GetWorldDetail` and `GetGamma` in the main table
//! `0x83de68` (records `0x83df10`..`0x83df68`): water detail, far clip, terrain mip, doodad
//! animation, texture LOD bias and base mip. Each getter ignores its arguments and pushes one
//! number; each setter takes `lua_isnumber(1)` (`0x6f34d0`) or raises its `Usage:` string, writes
//! one CVar through `CVar::Set` (`0x63df50`) and returns zero values.
//!
//! The stock window reaches only `Get/SetTerrainMip` and `Get/SetBaseMip` (`OptionsFrame.lua:28-29`,
//! `func` spelled as the verb); its far-clip row composes `Getfarclip`, which must stay nil.

use mlua::{Lua, MultiValue, Value};

use super::cvars::{format_f, sstr_to_int, write_cvar};
use super::Model;

/// The CVars these pairs read and write, which the host must register: a missing row would make a
/// setter warn and store nothing.
pub const VIDEO_PAIR_CVARS: [&str; 5] = [
    "farclip",
    "shadowLevel",
    "doodadAnim",
    "texLodBias",
    "baseMip",
];

/// How a getter answers, from the CVar record it looks up by name (`0x63de30`).
#[derive(Clone, Copy)]
enum Read {
    /// A constant, `GetWaterDetail`'s `0.0` (`0x488ec0`).
    Zero,
    /// The record's f32 (`fld [rec+0x24]`), widened.
    Float(&'static str),
    /// `1.0 − ` the record's integer (`fild [rec+0x28]`, `fsubr [0x8015b8]`).
    OneMinusInt(&'static str),
    /// `1.0 − ` the record's f32 (`fld [rec+0x24]`, `fsubr [0x8015b8]`).
    OneMinusFloat(&'static str),
}

/// How a setter writes, after the `lua_isnumber` gate and `lua_tonumber` (`0x6f3620`).
#[derive(Clone, Copy)]
enum Write {
    /// `SetWaterDetail` (`0x488ed0`): the gate, then nothing.
    Nothing,
    /// The number as "%f" (format `0x835160`).
    Float(&'static str),
    /// `1 − trunc(v)` as "%d" (`__ftol`, then `mov ecx,1; sub ecx,eax`, format `0x835154`).
    OneMinusTrunc(&'static str),
    /// `trunc(1.0 − v)` as "%d" (`fsubr [0x8015b8]`, then `__ftol`).
    TruncOneMinus(&'static str),
}

/// One pair: its two globals, the setter's usage string and the CVar's registered default, which a
/// bare VM without the host's rows reads as the record's value.
struct Pair {
    get: &'static str,
    set: &'static str,
    usage: &'static str,
    read: Read,
    write: Write,
    default: &'static str,
}

const PAIRS: [Pair; 6] = [
    // `0x488ec0`/`0x488ed0`, usage `0x842408`: no CVar behind either.
    Pair {
        get: "GetWaterDetail",
        set: "SetWaterDetail",
        usage: "Usage: SetWaterDetail(value)",
        read: Read::Zero,
        write: Write::Nothing,
        default: "0",
    },
    // `0x488f00`/`0x488f30` over `farclip` (`0x842428`), usage `0x842430`.
    Pair {
        get: "GetFarclip",
        set: "SetFarclip",
        usage: "Usage: SetFarclip(value)",
        read: Read::Float("farclip"),
        write: Write::Float("farclip"),
        default: "350",
    },
    // `0x488fb0`/`0x488fe0` over `shadowLevel` (`0x84244c`), usage `0x842458`: the slider shows
    // `1 − shadowLevel`, so High (1) is shadow mip 0.
    Pair {
        get: "GetTerrainMip",
        set: "SetTerrainMip",
        usage: "Usage: SetTerrainMip(value)",
        read: Read::OneMinusInt("shadowLevel"),
        write: Write::OneMinusTrunc("shadowLevel"),
        default: "1",
    },
    // `0x489060`/`0x489090` over `doodadAnim` (`0x842474`), usage `0x842480`.
    Pair {
        get: "GetDoodadAnim",
        set: "SetDoodadAnim",
        usage: "Usage: SetDoodadAnim(value)",
        read: Read::Float("doodadAnim"),
        write: Write::Float("doodadAnim"),
        default: "1",
    },
    // `0x489110`/`0x489140` over `texLodBias` (`0x84249c`), usage `0x8424a8`.
    Pair {
        get: "GetTexLodBias",
        set: "SetTexLodBias",
        usage: "Usage: SetTexLodBias(value)",
        read: Read::Float("texLodBias"),
        write: Write::Float("texLodBias"),
        default: "0.0",
    },
    // `0x489280`/`0x4892b0` over `baseMip` (`0x8424e4`), usage `0x8424ec`: the slider shows
    // `1 − baseMip`, so High (1) is base mip 0.
    Pair {
        get: "GetBaseMip",
        set: "SetBaseMip",
        usage: "Usage: SetBaseMip(value)",
        read: Read::OneMinusFloat("baseMip"),
        write: Write::TruncOneMinus("baseMip"),
        default: "0",
    },
];

/// `__ftol` (`0x40a2b0`) truncates toward zero into a qword, and the "%d" these setters format
/// reads its low dword. Out of range or NaN it stores the integer indefinite `0x8000000000000000`,
/// whose low dword is 0.
fn ftol_low(v: f64) -> i32 {
    if v.is_finite() && v.abs() < 2f64.powi(63) {
        v.trunc() as i64 as i32
    } else {
        0
    }
}

/// The record's value, or `default` when this VM has no such row.
fn record_value(model: &Model, name: &str, default: &str) -> String {
    model
        .cvars
        .get(&name.to_ascii_lowercase())
        .map_or_else(|| default.to_string(), |slot| slot.value.clone())
}

/// The record's `valueAsFloat` (`rec+0x24`), parsed from the value as an f32. A value that does
/// not parse reads 0.0; the setters and the host's registry store only numbers.
fn record_float(value: &str) -> f64 {
    f64::from(value.parse::<f32>().unwrap_or(0.0))
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();
    for pair in &PAIRS {
        let (read, default) = (pair.read, pair.default);
        g.set(
            pair.get,
            // Any arguments are ignored: the getter never calls `lua_gettop`.
            lua.create_function(move |lua, _: MultiValue| {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                Ok(match read {
                    Read::Zero => 0.0,
                    Read::Float(name) => record_float(&record_value(&model, name, default)),
                    Read::OneMinusInt(name) => {
                        1.0 - f64::from(sstr_to_int(&record_value(&model, name, default)))
                    }
                    Read::OneMinusFloat(name) => {
                        1.0 - record_float(&record_value(&model, name, default))
                    }
                })
            })?,
        )?;
        let (write, usage) = (pair.write, pair.usage);
        g.set(
            pair.set,
            lua.create_function(move |lua, value: Value| {
                // A number or a numeric string; anything else, or no argument, raises.
                let Some(v) = lua.coerce_number(value)? else {
                    return Err(mlua::Error::runtime(usage));
                };
                let (name, text) = match write {
                    Write::Nothing => return Ok(MultiValue::new()),
                    Write::Float(name) => (name, format_f(v)),
                    Write::OneMinusTrunc(name) => {
                        (name, 1i32.wrapping_sub(ftol_low(v)).to_string())
                    }
                    Write::TruncOneMinus(name) => (name, ftol_low(1.0 - v).to_string()),
                };
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                write_cvar(&mut model, name, text);
                // Zero return values, not nil (`xor eax,eax` at every `ret`).
                Ok(MultiValue::new())
            })?,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    /// A VM with the host's five rows at their reference defaults.
    fn script() -> UiScript {
        let s = UiScript::new().unwrap();
        s.register_cvars([
            ("farclip", "350"),
            ("shadowLevel", "1"),
            ("doodadAnim", "1"),
            ("texLodBias", "0.0"),
            ("baseMip", "0"),
        ]);
        s
    }

    #[test]
    fn every_pair_has_the_reference_shape() {
        let s = script();
        for get in [
            "GetWaterDetail",
            "GetFarclip",
            "GetTerrainMip",
            "GetDoodadAnim",
            "GetTexLodBias",
            "GetBaseMip",
        ] {
            assert_eq!(s.arity(&format!("{get}()")).unwrap(), 1, "{get}");
            assert_eq!(
                s.eval::<String>(&format!("return type({get}(7, 'x'))"))
                    .unwrap(),
                "number",
                "{get} ignores its arguments and pushes a number"
            );
        }
        for (set, usage) in [
            ("SetWaterDetail", "Usage: SetWaterDetail(value)"),
            ("SetFarclip", "Usage: SetFarclip(value)"),
            ("SetTerrainMip", "Usage: SetTerrainMip(value)"),
            ("SetDoodadAnim", "Usage: SetDoodadAnim(value)"),
            ("SetTexLodBias", "Usage: SetTexLodBias(value)"),
            ("SetBaseMip", "Usage: SetBaseMip(value)"),
        ] {
            assert_eq!(s.arity(&format!("{set}(1)")).unwrap(), 0, "{set}");
            assert_eq!(s.arity(&format!("{set}(\"1\")")).unwrap(), 0, "{set}");
            for bad in ["", "nil", "\"x\"", "{}", "true"] {
                let e = s.run(&format!("{set}({bad})")).unwrap_err().to_string();
                assert!(e.contains(usage), "{set}({bad}) raised {e}");
            }
        }
        // The stock window's lower-case spellings are not bindings.
        assert!(s
            .eval::<bool>("return Getfarclip == nil and Setfarclip == nil")
            .unwrap());
    }

    #[test]
    fn the_getters_read_their_records_at_the_reference_defaults() {
        let s = script();
        let get = |e: &str| s.eval::<f64>(&format!("return {e}()")).unwrap();
        assert_eq!(get("GetWaterDetail"), 0.0);
        assert_eq!(get("GetFarclip"), 350.0);
        // `shadowLevel` "1" → `1 − 1`; `baseMip` "0" → `1 − 0`: both sliders at High.
        assert_eq!(get("GetTerrainMip"), 0.0);
        assert_eq!(get("GetBaseMip"), 1.0);
        assert_eq!(get("GetDoodadAnim"), 1.0);
        assert_eq!(get("GetTexLodBias"), 0.0);
        // A bare VM without the rows answers the registered defaults.
        let bare = UiScript::new().unwrap();
        assert_eq!(bare.eval::<f64>("return GetFarclip()").unwrap(), 350.0);
        assert_eq!(bare.eval::<f64>("return GetTerrainMip()").unwrap(), 0.0);
    }

    #[test]
    fn the_setters_write_the_reference_text() {
        let s = script();
        let set = |call: &str, cvar: &str| {
            s.run(call).unwrap();
            s.cvar(cvar).unwrap()
        };
        // "%f" of the number, as `SetGamma` writes.
        assert_eq!(set("SetFarclip(500)", "farclip"), "500.000000");
        assert_eq!(s.eval::<f64>("return GetFarclip()").unwrap(), 500.0);
        assert_eq!(set("SetDoodadAnim(0)", "doodadAnim"), "0.000000");
        assert_eq!(set("SetTexLodBias(-0.5)", "texLodBias"), "-0.500000");
        assert_eq!(s.eval::<f64>("return GetTexLodBias()").unwrap(), -0.5);
        // `SetTerrainMip`: truncate, then `1 − n`.
        assert_eq!(set("SetTerrainMip(1)", "shadowLevel"), "0");
        assert_eq!(s.eval::<f64>("return GetTerrainMip()").unwrap(), 1.0);
        assert_eq!(set("SetTerrainMip(0.9)", "shadowLevel"), "1");
        // `SetBaseMip`: `1 − v`, then truncate, so 0.5 is `trunc(0.5)` = 0, not `1 − 0`.
        assert_eq!(set("SetBaseMip(0)", "baseMip"), "1");
        assert_eq!(s.eval::<f64>("return GetBaseMip()").unwrap(), 0.0);
        assert_eq!(set("SetBaseMip(0.5)", "baseMip"), "0");
        assert_eq!(set("SetBaseMip(\"0\")", "baseMip"), "1");
        // `SetWaterDetail` checks its argument and writes nothing.
        s.run("SetWaterDetail(3)").unwrap();
        assert_eq!(s.eval::<f64>("return GetWaterDetail()").unwrap(), 0.0);
    }

    #[test]
    fn ftol_keeps_the_low_dword_of_the_truncation() {
        assert_eq!(super::ftol_low(2.9), 2);
        assert_eq!(super::ftol_low(-2.9), -2);
        assert_eq!(super::ftol_low(4_294_967_297.0), 1);
        assert_eq!(super::ftol_low(f64::NAN), 0);
        assert_eq!(super::ftol_low(1e300), 0);
    }
}
