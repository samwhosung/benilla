//! The `MessageFrame` methods over [`MessageFrameState`](crate::widget::MessageFrameState):
//! `CSimpleMessageFrame` (ctor `0x785640`), `UIErrorsFrame`'s class and what
//! `CreateFrame("MessageFrame")` makes. Its `AddMessage(text [, r, g, b [, a]])` (`0x795590`) takes
//! a real alpha and reads nothing after it, where the scrolling class's (`0x792900`) takes an id
//! there and forces alpha opaque. `SetInsertMode` and `GetInsertMode` (`0x794ed0`, `0x794ff0`)
//! exist on this class only.

use mlua::{Lua, Table, Value};

use crate::script::object::frame_handle_of;
use crate::script::Model;
use crate::widget::{InsertMode, KindState, MessageFrameState};

/// Registry key of the MessageFrame method table (the MAXCSTACK discipline).
pub(crate) const REG_MESSAGEFRAME_METHODS: &str = "__benilla_plain_messageframe_methods";

/// Run `f` over a MessageFrame's state under one short borrow; any other frame, reachable only
/// through a misapplied method table, is an error.
fn with_mf<T>(
    lua: &Lua,
    this: &Table,
    f: impl FnOnce(&mut MessageFrameState) -> T,
) -> mlua::Result<T> {
    let h = frame_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let frame = model
        .arena
        .frame_mut(h)
        .ok_or_else(|| mlua::Error::runtime("stale frame handle"))?;
    match &mut frame.kind_state {
        KindState::Message(mf) => Ok(f(mf)),
        _ => Err(mlua::Error::runtime("not a MessageFrame")),
    }
}

/// A colour argument as f32, 0 for anything but a number.
fn num_f32(v: &Value) -> f32 {
    match v {
        Value::Number(n) => *n as f32,
        Value::Integer(i) => *i as f32,
        _ => 0.0,
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;

    // The justify quartet, on both classes ([`super::install_justify`]).
    super::install_justify(lua, &m, "MessageFrame")?;

    // AddMessage(text [, r, g, b [, a]]) (`0x795590`). An unusable text is a silent no-op
    // ([`super::message_text`]). r, g and b count only as a trio, else white, and a bad colour
    // still adds the line in white (`0x79581c`); the alpha defaults to 1.0 (`0x795752`). The
    // closure's arity drops a sixth argument, as the binding reads five: addons that pass a hold
    // time there get the frame's `displayDuration`, as on the reference.
    m.set(
        "AddMessage",
        lua.create_function(
            |lua, (this, text, r, g, b, a): (Table, Value, Value, Value, Value, Value)| {
                let Some(text) = super::message_text(lua, &text) else {
                    return Ok(());
                };
                let has_rgb = !matches!(
                    (&r, &g, &b),
                    (Value::Nil, _, _) | (_, Value::Nil, _) | (_, _, Value::Nil)
                );
                let (r, g, b) = if has_rgb {
                    (num_f32(&r), num_f32(&g), num_f32(&b))
                } else {
                    (1.0, 1.0, 1.0)
                };
                // The alpha counts only with the trio, read in the same parse.
                let a = match (&a, has_rgb) {
                    (Value::Number(_) | Value::Integer(_), true) => num_f32(&a),
                    _ => 1.0,
                };
                with_mf(lua, &this, |mf| mf.add(text, r, g, b, a))
            },
        )?,
    )?;

    m.set(
        "Clear",
        lua.create_function(|lua, this: Table| with_mf(lua, &this, MessageFrameState::clear))?,
    )?;

    // SetInsertMode (`0x794ed0`) compares against "BOTTOM" (`0x871404`), so anything else is
    // TOP; the ctor default is BOTTOM.
    m.set(
        "SetInsertMode",
        lua.create_function(|lua, (this, mode): (Table, Value)| {
            let mode = match &mode {
                Value::String(s) => {
                    if s.to_str()?.trim().eq_ignore_ascii_case("BOTTOM") {
                        InsertMode::Bottom
                    } else {
                        InsertMode::Top
                    }
                }
                // A non-string fails the compare too.
                _ => InsertMode::Top,
            };
            with_mf(lua, &this, |mf| mf.insert_mode = mode)
        })?,
    )?;
    m.set(
        "GetInsertMode",
        lua.create_function(|lua, this: Table| {
            with_mf(lua, &this, |mf| match mf.insert_mode {
                InsertMode::Top => "TOP",
                InsertMode::Bottom => "BOTTOM",
            })
        })?,
    )?;

    // The fade accessors, as on the scrolling class; XML's `displayDuration` is `TimeVisible`.
    m.set(
        "SetFading",
        lua.create_function(|lua, (this, on): (Table, Value)| {
            let on = !matches!(on, Value::Nil | Value::Boolean(false));
            with_mf(lua, &this, |mf| mf.fading_enabled = on)
        })?,
    )?;
    // 1 or nil, the 1.12 predicate shape (`0x795170`).
    m.set(
        "GetFading",
        lua.create_function(|lua, this: Table| {
            with_mf(lua, &this, |mf| {
                crate::script::binding_abi::flag(mf.fading_enabled)
            })
        })?,
    )?;
    m.set(
        "SetTimeVisible",
        lua.create_function(|lua, (this, s): (Table, f32)| {
            with_mf(lua, &this, |mf| mf.time_visible = s.max(0.0))
        })?,
    )?;
    m.set(
        "GetTimeVisible",
        lua.create_function(|lua, this: Table| with_mf(lua, &this, |mf| mf.time_visible))?,
    )?;
    m.set(
        "SetFadeDuration",
        lua.create_function(|lua, (this, s): (Table, f32)| {
            with_mf(lua, &this, |mf| mf.fade_duration = s.max(0.0))
        })?,
    )?;
    m.set(
        "GetFadeDuration",
        lua.create_function(|lua, this: Table| with_mf(lua, &this, |mf| mf.fade_duration))?,
    )?;

    // ── the shared font block ───────────────────────────────────────────────────────────────
    // The ten font verbs are on this class's own table (`GetShadowColor` `0x794810`), as on
    // FontString, Font, EditBox, ScrollingMessageFrame and SimpleHTML; Button's (`0x879d00`) has
    // none.
    crate::script::font_block::install(
        lua,
        &m,
        |lua, this| {
            let h = frame_handle_of(lua, this)?;
            super::ensure_font_region(lua, h)
                .ok_or_else(|| mlua::Error::runtime("not a MessageFrame"))
        },
        "MessageFrame",
    )?;

    lua.set_named_registry_value(REG_MESSAGEFRAME_METHODS, m)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    /// A named frame's line count, off the state: `GetNumMessages` is not a MessageFrame method.
    fn num_messages(s: &UiScript, name: &str) -> usize {
        let m = s.model_ref();
        let h = m.arena.lookup(name).expect("named frame");
        m.arena
            .frame(h)
            .and_then(|f| f.kind_state.message_lines())
            .map_or(0, |l| l.len())
    }

    /// A sixth argument, which some addons pass as a hold time, changes nothing.
    #[test]
    fn the_sixth_addmessage_argument_is_ignored_not_a_holdtime() {
        let mut s = UiScript::new().unwrap();
        s.run("CreateFrame('MessageFrame', 'MF')").unwrap();
        s.run("MF:SetTimeVisible(1)").unwrap();
        s.run("MF:SetFadeDuration(0)").unwrap();
        s.run("MF:AddMessage('held', 1.0, 1.0, 1.0, 1.0, 999)")
            .unwrap();
        s.run("MF:AddMessage('plain', 1.0, 1.0, 1.0, 1.0)").unwrap();
        assert_eq!(num_messages(&s, "MF"), 2);
        // Past timeVisible with no ramp, both retire together.
        s.tick(1.1);
        s.tick(0.1);
        assert_eq!(
            num_messages(&s, "MF"),
            0,
            "the 6th arg must not extend a message's life"
        );
    }

    /// Only an unusable text is silent (`0x795590` jumps to its epilogue): a bad colour still adds
    /// the line, and the receiver check still raises.
    #[test]
    fn addmessage_swallows_a_text_it_cannot_use_and_only_the_text() {
        let s = UiScript::new().unwrap();
        s.run("CreateFrame('MessageFrame', 'MF')").unwrap();

        // `lua_isstring` (`0x6f3510`) passes strings and numbers only; the rest jump to `0x79582b`.
        for bad in ["nil", "", "true", "{}", "print"] {
            s.run(&format!("MF:AddMessage({bad})"))
                .unwrap_or_else(|e| panic!("AddMessage({bad}) must not raise: {e}"));
        }
        // The empty string has its own check (`0x79564b`).
        s.run("MF:AddMessage('')").unwrap();
        assert_eq!(
            num_messages(&s, "MF"),
            0,
            "nil, absent, boolean, table, function and \"\" each add NOTHING"
        );

        // A number is a text, retagged in place (`0x6f7c80`).
        s.run("MF:AddMessage(42)").unwrap();
        assert_eq!(num_messages(&s, "MF"), 1, "a number is a text");

        // A bad colour jumps to `0x79581c`, which still adds the line in the white staged at
        // `0x795658`.
        s.run("MF:AddMessage('kept', {}, nil, 'x')").unwrap();
        assert_eq!(
            num_messages(&s, "MF"),
            2,
            "a bad colour still adds the line"
        );

        // The receiver check still raises (`0x847ef8`).
        assert!(
            s.run("MF.AddMessage('dot')").is_err(),
            "a '.'-instead-of-':' call still raises"
        );
    }

    #[test]
    fn set_insert_mode_is_messageframe_only() {
        let s = UiScript::new().unwrap();
        s.run(
            "CreateFrame('MessageFrame', 'MF')\n\
             CreateFrame('Frame', 'PlainF')\n\
             CreateFrame('ScrollingMessageFrame', 'SMF')",
        )
        .unwrap();
        assert_eq!(
            s.eval::<String>("return type(MF.SetInsertMode)").unwrap(),
            "function"
        );
        assert_eq!(
            s.eval::<String>("return type(PlainF.SetInsertMode)")
                .unwrap(),
            "nil",
            "a plain Frame must not quack like a MessageFrame"
        );
        assert_eq!(
            s.eval::<String>("return type(SMF.SetInsertMode)").unwrap(),
            "nil",
            "the scrolling class has no SetInsertMode binding at all"
        );
        assert_eq!(
            s.eval::<String>("return MF:GetInsertMode()").unwrap(),
            "BOTTOM"
        );
        s.run("MF:SetInsertMode('TOP')").unwrap();
        assert_eq!(
            s.eval::<String>("return MF:GetInsertMode()").unwrap(),
            "TOP"
        );
    }

    /// The fifth argument is alpha on a MessageFrame and an id on a ScrollingMessageFrame.
    #[test]
    fn messageframe_addmessage_is_not_the_scrolling_ones() {
        let s = UiScript::new().unwrap();
        s.run(
            "CreateFrame('MessageFrame', 'MF')\n\
             CreateFrame('ScrollingMessageFrame', 'SMF')",
        )
        .unwrap();
        assert!(
            !s.eval::<bool>("return MF.AddMessage == SMF.AddMessage")
                .unwrap(),
            "one shared AddMessage would give the wrong meaning to the 5th arg on one of them"
        );
        s.run("MF:AddMessage('half', 1, 1, 1, 0.5)").unwrap();
        s.run("SMF:AddMessage('ident', 1, 1, 1, 42)").unwrap();
        let alphas = {
            let m = s.model_ref();
            let pick = |name: &str| {
                let h = m.arena.lookup(name).unwrap();
                m.arena
                    .frame(h)
                    .unwrap()
                    .kind_state
                    .message_lines()
                    .unwrap()[0]
                    .alpha
            };
            (pick("MF"), pick("SMF"))
        };
        assert!(
            (alphas.0 - 128.0 / 255.0).abs() < 1e-6,
            "MessageFrame's 5th arg is alpha, quantized round-half-up: {alphas:?}"
        );
        assert_eq!(
            alphas.1, 1.0,
            "ScrollingMessageFrame's 5th arg is an id; its alpha is forced opaque"
        );
    }

    #[test]
    fn insert_mode_picks_the_growth_edge() {
        let bands = |mode: &str| {
            let mut s = UiScript::new().unwrap();
            s.set_screen_size(800.0, 600.0);
            s.run(
                "local f = CreateFrame('MessageFrame', 'MF')\n\
                 f:SetPoint('BOTTOMLEFT', 0, 100)\n\
                 f:SetWidth(400)\n\
                 f:SetHeight(48)",
            )
            .unwrap();
            s.run(&format!("MF:SetInsertMode('{mode}')")).unwrap();
            for t in ["one", "two", "three"] {
                s.run(&format!("MF:AddMessage('{t}', 1, 1, 1)")).unwrap();
            }
            s.resolve();
            let mut v: Vec<(String, f32)> = s
                .extract()
                .iter()
                .filter_map(|q| match (&q.content, q.rect) {
                    (crate::script::QuadContent::Text { text: Some(t), .. }, Some(r)) => {
                        Some((t.clone(), r.bottom))
                    }
                    _ => None,
                })
                .collect();
            v.sort_by(|a, b| b.1.total_cmp(&a.1)); // top row first
            v
        };
        // Default pitch 14, frame [100, 148).
        let top = bands("TOP");
        assert_eq!(
            top.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(),
            ["three", "two", "one"],
            "TOP: newest on the frame's top row, older stepping down"
        );
        assert!((top[0].1 - 134.0).abs() < 0.01, "newest hangs off fr.top");
        let bottom = bands("BOTTOM");
        assert_eq!(
            bottom.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(),
            ["one", "two", "three"],
            "BOTTOM: newest on the frame's bottom row, older stepping up"
        );
        assert!(
            (bottom[2].1 - 100.0).abs() < 0.01,
            "newest sits on fr.bottom"
        );
    }

    /// No `maxLines`: the cap is what fits vertically, applied at the tick.
    #[test]
    fn the_cap_is_what_fits_vertically() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        s.run(
            "local f = CreateFrame('MessageFrame', 'MF')\n\
             f:SetPoint('BOTTOMLEFT', 0, 0)\n\
             f:SetWidth(400)\n\
             f:SetHeight(42)\n\
             f:SetFading(false)",
        )
        .unwrap();
        for n in 0..9 {
            s.run(&format!("MF:AddMessage('m{n}', 1, 1, 1)")).unwrap();
        }
        s.resolve();
        s.tick(0.1);
        assert_eq!(num_messages(&s, "MF"), 3, "42px / 14px pitch = 3 rows");
        s.resolve();
        let texts: Vec<String> = s
            .extract()
            .iter()
            .filter_map(|q| match &q.content {
                crate::script::QuadContent::Text { text: Some(t), .. } => Some(t.clone()),
                _ => None,
            })
            .collect();
        assert!(texts.contains(&"m8".to_string()) && !texts.contains(&"m0".to_string()));
    }

    /// A faded line is freed, where the scrolling class keeps its rows.
    #[test]
    fn a_finished_line_is_retired_and_clear_empties_now() {
        let mut s = UiScript::new().unwrap();
        s.run("CreateFrame('MessageFrame', 'MF')").unwrap();
        s.run("MF:SetTimeVisible(0.5)").unwrap();
        s.run("MF:SetFadeDuration(2)").unwrap();
        s.run("MF:AddMessage('x', 1, 1, 1)").unwrap();
        s.tick(0.6); // spends phase 1
        s.tick(1.0); // mid-ramp: still alive, alpha down
        assert_eq!(num_messages(&s, "MF"), 1);
        s.tick(1.2); // ramp done → retired
        assert_eq!(num_messages(&s, "MF"), 0);

        s.run("MF:AddMessage('a', 1, 1, 1)").unwrap();
        s.run("MF:AddMessage('b', 1, 1, 1)").unwrap();
        assert_eq!(num_messages(&s, "MF"), 2);
        s.run("MF:Clear()").unwrap();
        assert_eq!(num_messages(&s, "MF"), 0);
    }

    /// The ctor defaults: timeVisible (`0x81cc2c`) and fadeDuration (`0x81cc30`).
    #[test]
    fn ctor_defaults_and_the_fade_accessors() {
        let s = UiScript::new().unwrap();
        s.run("CreateFrame('MessageFrame', 'MF')").unwrap();
        assert!(s.eval::<bool>("return MF:GetFading()").unwrap());
        assert_eq!(s.eval::<f32>("return MF:GetTimeVisible()").unwrap(), 10.0);
        assert_eq!(s.eval::<f32>("return MF:GetFadeDuration()").unwrap(), 3.0);
        s.run("MF:SetTimeVisible(5) MF:SetFadeDuration(1) MF:SetFading(false)")
            .unwrap();
        assert_eq!(s.eval::<f32>("return MF:GetTimeVisible()").unwrap(), 5.0);
        assert_eq!(s.eval::<f32>("return MF:GetFadeDuration()").unwrap(), 1.0);
        assert!(!s.eval::<bool>("return MF:GetFading()").unwrap());
    }
}
