//! The EditBox Lua method table (`REG_EDITBOX_METHODS`), consulted before the shared frame
//! table; each method routes through the parent module's primitives.

use mlua::{Lua, Table, Value};

use crate::script::object::frame_handle_of;
use crate::widget::EditBoxState;

use super::{
    clear_focus_handle, highlight_text, insert, set_focus_handle, set_text, set_text_insets,
    with_eb, REG_EDITBOX_METHODS,
};

fn with_editbox<T>(
    lua: &Lua,
    this: &Table,
    f: impl FnOnce(&mut EditBoxState) -> T,
) -> mlua::Result<T> {
    let h = frame_handle_of(lua, this)?;
    with_eb(lua, h, f).ok_or_else(|| mlua::Error::runtime("not an EditBox"))
}

pub(in crate::script) fn install(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;

    m.set(
        "SetText",
        lua.create_function(|lua, (this, s): (Table, Option<mlua::Value>)| {
            let s = crate::script::binding_abi::text_arg(lua, s)?;
            let h = frame_handle_of(lua, &this)?;
            // SetText keeps a history browse going: the chat parser rewrites each recalled slash
            // line, and ending the browse there would send every Up back to the newest entry.
            // Typed edits, AddHistoryLine and focus gain end it.
            set_text(lua, h, &s.unwrap_or_default());
            Ok(())
        })?,
    )?;
    m.set(
        "GetText",
        lua.create_function(|lua, this: Table| with_editbox(lua, &this, |eb| eb.text.clone()))?,
    )?;
    // SetNumber is SetText: `0x798690` and `0x7984c0` are byte-identical but for the usage
    // string. Its gate is `lua_isstring` (`0x6f3510`), so a string is set verbatim, never parsed,
    // and nil, a boolean, a table or no argument raises the usage string. On a numeric box `-5` or
    // `0.8` leaves it empty: the sign or point fails the digit test after the clear has run.
    m.set(
        "SetNumber",
        lua.create_function(|lua, (this, v): (Table, Value)| {
            let text = match &v {
                Value::Integer(i) => crate::script::binding_abi::lua_number_text(*i as f64),
                Value::Number(n) => crate::script::binding_abi::lua_number_text(*n),
                Value::String(s) => s.to_str()?.to_string(),
                _ => return Err(mlua::Error::runtime("Usage: EditBox:SetNumber(number)")),
            };
            let h = frame_handle_of(lua, &this)?;
            set_text(lua, h, &text);
            Ok(())
        })?,
    )?;
    // GetNumber: the real text as a number, else 0. The reference (`0x798790`) is atof, which also
    // reads the leading number of a text like "12abc".
    m.set(
        "GetNumber",
        lua.create_function(|lua, this: Table| {
            let text = with_editbox(lua, &this, |eb| eb.text.clone())?;
            Ok(text.trim().parse::<f64>().unwrap_or(0.0))
        })?,
    )?;
    // Insert: a nil is a no-op, not an error, since the reference reads the argument through
    // `lua_tostring`, which answers NULL for nil; stock `LootFrame.lua:152` inserts a coin row's
    // nil link unguarded. A number inserts its digits.
    m.set(
        "Insert",
        lua.create_function(|lua, (this, s): (Table, Option<mlua::Value>)| {
            let s = crate::script::binding_abi::text_arg(lua, s)?;
            let h = frame_handle_of(lua, &this)?;
            if let Some(s) = s {
                insert(lua, h, &s, true);
            }
            Ok(())
        })?,
    )?;

    m.set(
        "SetFocus",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            set_focus_handle(lua, h);
            Ok(())
        })?,
    )?;
    m.set(
        "ClearFocus",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            clear_focus_handle(lua, h);
            Ok(())
        })?,
    )?;
    // No `HasFocus`: 1.12's EditBox table has these two setters and no getter, and addons keep
    // their own flag.

    // GetInputLanguage/ToggleInputLanguage (`0x799550`/`0x799610`): the IME's input language, not
    // the chat language. With no IME, as here, the answer is "ROMAN" and the toggle does nothing.
    m.set(
        "GetInputLanguage",
        lua.create_function(|lua, this: Table| {
            frame_handle_of(lua, &this)?;
            Ok("ROMAN")
        })?,
    )?;
    m.set(
        "ToggleInputLanguage",
        lua.create_function(|lua, this: Table| {
            frame_handle_of(lua, &this)?;
            Ok(())
        })?,
    )?;

    // HighlightText([start [, end]]): the defaults (0, -1) select all.
    m.set(
        "HighlightText",
        lua.create_function(
            |lua, (this, start, end): (Table, Option<i64>, Option<i64>)| {
                let h = frame_handle_of(lua, &this)?;
                highlight_text(lua, h, start.unwrap_or(0), end.unwrap_or(-1));
                Ok(())
            },
        )?,
    )?;

    m.set(
        // `GetMaxLetters` (`0x79929f`) reads the field `SetMaxLetters` writes.
        "GetMaxLetters",
        lua.create_function(|lua, this: Table| {
            with_editbox(lua, &this, |eb| eb.max_letters as i64)
        })?,
    )?;
    m.set(
        // `SetMaxBytes` (`0x798f30`) has `SetMaxLetters`'s exact count gate (`0x798fbc`) and raw
        // coercion, but its no-limit value is -1 (`0x799012`; the ctor writes -1 at `0x7799df`):
        // a non-positive argument is unlimited, a positive one caps the bytes.
        "SetMaxBytes",
        lua.create_function(|lua, (this, args): (Table, mlua::MultiValue)| {
            let args: Vec<Value> = args.into_iter().collect();
            if args.len() != 1 {
                return Err(mlua::Error::runtime(
                    "Usage: <unnamed>:SetMaxBytes(maxBytes)",
                ));
            }
            let n = crate::script::binding_abi::coerced_number(lua, args.first().cloned());
            let n = n as i64;
            with_editbox(lua, &this, |eb| {
                eb.max_bytes = (n > 0).then_some(n as usize);
            })
        })?,
    )?;
    m.set(
        // -1 while unlimited, the reference's stored sentinel.
        "GetMaxBytes",
        lua.create_function(|lua, this: Table| {
            with_editbox(lua, &this, |eb| eb.max_bytes.map_or(-1, |n| n as i64))
        })?,
    )?;
    m.set(
        // `SetMaxLetters` (`0x799110`) checks the count exactly, `lua_gettop` (`0x6f3070`) against
        // 2 at `0x79919c`, and not the type: no argument or two raise `Usage:`, nil or `{}` store
        // 0, `"12"` stores 12. 0 is no limit: the insert skips its trim on zero (`0x77c085`).
        "SetMaxLetters",
        lua.create_function(|lua, (this, args): (Table, mlua::MultiValue)| {
            let args: Vec<Value> = args.into_iter().collect();
            if args.len() != 1 {
                return Err(mlua::Error::runtime(
                    "Usage: <unnamed>:SetMaxLetters(maxLetters)",
                ));
            }
            let n = crate::script::binding_abi::coerced_number(lua, args.first().cloned());
            // Truncated toward zero like `__ftol`; the reference stores a negative as is, but its
            // trim only acts above 0, so the clamp changes nothing.
            with_editbox(lua, &this, |eb| eb.max_letters = (n as i64).max(0) as usize)
        })?,
    )?;
    // The sent-line history (`historyLines`): `ChatEdit_AddHistory` pushes each line, Up and Down
    // recall them.
    m.set(
        "AddHistoryLine",
        lua.create_function(|lua, (this, line): (Table, Option<String>)| {
            with_editbox(lua, &this, |eb| {
                eb.add_history_line(line.as_deref().unwrap_or(""));
            })
        })?,
    )?;
    m.set(
        "SetHistoryLines",
        lua.create_function(|lua, (this, n): (Table, i64)| {
            with_editbox(lua, &this, |eb| {
                eb.history_max = n.max(0) as usize;
                let over = eb.history.len().saturating_sub(eb.history_max);
                if over > 0 {
                    eb.history.drain(..over);
                }
            })
        })?,
    )?;
    m.set(
        "GetHistoryLines",
        lua.create_function(|lua, this: Table| {
            with_editbox(lua, &this, |eb| eb.history_max as i64)
        })?,
    )?;
    // SetBlinkSpeed/GetBlinkSpeed: the caret half-period (XML `blinkSpeed`, 0.5 s by default).
    m.set(
        "SetBlinkSpeed",
        lua.create_function(|lua, (this, s): (Table, f32)| {
            with_editbox(lua, &this, |eb| eb.blink_period = s)
        })?,
    )?;
    m.set(
        "GetBlinkSpeed",
        lua.create_function(|lua, this: Table| with_editbox(lua, &this, |eb| eb.blink_period))?,
    )?;
    m.set(
        "SetTextInsets",
        lua.create_function(
            |lua,
             (this, l, r, t, b): (
                Table,
                Option<f32>,
                Option<f32>,
                Option<f32>,
                Option<f32>,
            )| {
                let h = frame_handle_of(lua, &this)?;
                set_text_insets(
                    lua,
                    h,
                    l.unwrap_or(0.0),
                    r.unwrap_or(0.0),
                    t.unwrap_or(0.0),
                    b.unwrap_or(0.0),
                );
                Ok(())
            },
        )?,
    )?;
    m.set(
        "GetTextInsets",
        lua.create_function(|lua, this: Table| {
            let ins = with_editbox(lua, &this, |eb| eb.text_insets)?;
            Ok((ins[0], ins[1], ins[2], ins[3]))
        })?,
    )?;
    // GetNumLetters (`0x7992c0`): the class array's letters (`0x77bc80`), classes 2, 3 and 6 only,
    // so escapes are free and a 43-byte item link counts 9.
    m.set(
        "GetNumLetters",
        lua.create_function(|lua, this: Table| {
            with_editbox(lua, &this, |eb| {
                crate::markup::ClassMap::new(&eb.text).num_letters() as i64
            })
        })?,
    )?;

    // The XML's fifth flag, `ignoreArrows`, is `SetAltArrowKeyMode` in Lua; 1.12 has no
    // `SetIgnoreArrows` (the 48-entry table `[0x87bb68, 0x87bce8)`).
    //
    // SetAltArrowKeyMode (`0x7996e0`) reads `GetBoolOrDefault` (`0x6f1c10`) with default 1: no
    // argument enables, nil disables, a number truncates (0 and 0.5 off, -1 on), `""` takes the
    // default and `"0"` disables. GetAltArrowKeyMode (`0x799790`) answers the number 1 or nil,
    // never a boolean (`0x799815`).
    m.set(
        "SetAltArrowKeyMode",
        lua.create_function(|lua, (this, args): (Table, mlua::MultiValue)| {
            // `MultiValue`, since a `Value` cannot tell no argument (on) from nil (off).
            let args: Vec<Value> = args.into_iter().collect();
            let on = crate::script::binding_abi::bool_or_default(args.first(), true);
            with_editbox(lua, &this, |eb| eb.alt_arrow_key_mode = on)?;
            Ok(())
        })?,
    )?;
    m.set(
        "GetAltArrowKeyMode",
        lua.create_function(|lua, this: Table| {
            let on = with_editbox(lua, &this, |eb| eb.alt_arrow_key_mode)?;
            Ok(if on { Value::Integer(1) } else { Value::Nil })
        })?,
    )?;

    for (name, set) in flag_setters() {
        let refresh_justify = name == "SetMultiLine";
        m.set(
            name,
            lua.create_function(move |lua, (this, v): (Table, Value)| {
                let on = !matches!(v, Value::Nil | Value::Boolean(false));
                with_editbox(lua, &this, |eb| set(eb, on))?;
                if refresh_justify {
                    // A multi-line box anchors its text TOP, else MIDDLE, and the loader wires the
                    // `<FontString>` before the flags, so re-seat it.
                    super::refresh_text_region_justify(lua, &this)?;
                }
                Ok(())
            })?,
        )?;
    }

    install_font_block(lua, &m)?;

    lua.set_named_registry_value(REG_EDITBOX_METHODS, m)?;
    Ok(())
}

/// The font block, entries #0-#15 of the EditBox's own 48-entry method table (`0x87bb68`, count
/// at `0x799ab5`); 1.12 has no `FontInstance` class, so each text-bearing kind declares it. Ten
/// come from [`super::super::font_block`]. `SetSpacing`/`GetSpacing` (#10-#11) are not installed:
/// nothing renders line spacing, and a missing method raises by name. The justify four (#12-#15)
/// act on the box's own [`EditBoxState::justify`] word, not its text region.
fn install_font_block(lua: &Lua, m: &Table) -> mlua::Result<()> {
    // Every font method acts on the box's implicit FontString (`[this+0x324]`, our
    // `text_region`), created on demand.
    crate::script::font_block::install(
        lua,
        m,
        |lua, this| {
            let h = frame_handle_of(lua, this)?;
            super::ensure_text_region(lua, h).ok_or_else(|| mlua::Error::runtime("not an EditBox"))
        },
        "EditBox",
    )?;

    // SetJustifyH/SetJustifyV return nothing. An unknown token raises `Usage:` (`0x87c77c`); a
    // token of the other axis parses and masks to nothing, so `SetJustifyH("TOP")` clears justifyH
    // and `GetJustifyH()` answers "UNKNOWN".
    for (name, mask) in [
        ("SetJustifyH", EditBoxState::JUSTIFY_H_MASK),
        ("SetJustifyV", EditBoxState::JUSTIFY_V_MASK),
    ] {
        m.set(
            name,
            lua.create_function(move |lua, (this, token): (Table, String)| {
                let bits = EditBoxState::justify_bit(&token).ok_or_else(|| {
                    mlua::Error::runtime(format!("Usage: <EditBox>:{name}(\"justify\")"))
                })?;
                with_editbox(lua, &this, |eb| eb.set_justify_axis(mask, bits))
            })?,
        )?;
    }
    // GetJustifyH/GetJustifyV: one string, the first set bit's token in the reference's table
    // order, else "UNKNOWN".
    for (name, mask) in [
        ("GetJustifyH", EditBoxState::JUSTIFY_H_MASK),
        ("GetJustifyV", EditBoxState::JUSTIFY_V_MASK),
    ] {
        m.set(
            name,
            lua.create_function(move |lua, this: Table| {
                with_editbox(lua, &this, |eb| eb.justify_token(mask))
            })?,
        )?;
    }
    Ok(())
}

/// A `Set<Flag>` method name and the field write it drives.
type FlagSetter = (&'static str, fn(&mut EditBoxState, bool));

/// The four config-flag setters, each taking Lua truthiness.
fn flag_setters() -> [FlagSetter; 4] {
    [
        ("SetAutoFocus", |eb, on| eb.auto_focus = on),
        ("SetNumeric", |eb, on| eb.numeric = on),
        ("SetPassword", |eb, on| eb.password = on),
        ("SetMultiLine", |eb, on| eb.multi_line = on),
    ]
}
