//! The EditBox Lua method surface — `install` builds the method table
//! (`REG_EDITBOX_METHODS`) the dispatcher consults before the shared frame table for EditBox
//! frames: text/cursor/selection accessors, focus, history, the config setters, and the blink
//! dial. Every method routes through the mother module's primitives (`set_text`,
//! `highlight_text`, …), so the byte-verified law lives exactly once.

use mlua::{Lua, Table, Value};

use crate::script::object::frame_handle_of;
use crate::widget::EditBoxState;

use super::{
    clear_focus_handle, highlight_text, insert, set_focus_handle, set_text, set_text_insets,
    with_eb, REG_EDITBOX_METHODS,
};

/// Run `f` over a frame's EditBox state for a Lua method call; errors if `this` is not a live EditBox.
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
            // Programmatic SetText KEEPS a history browse in progress: the chat live parse
            // rewrites the box on every recalled slash line ("/s hi" → Say + "hi"), and ending
            // the browse there reset every UP to the newest entry — "history only goes back 1"
            // (director's report, 2026-07-26). The browse ends on typed edits, AddHistoryLine,
            // and focus gain (`set_focus_handle`) — the fresh-session reset.
            set_text(lua, h, &s.unwrap_or_default());
            Ok(())
        })?,
    )?;
    m.set(
        "GetText",
        lua.create_function(|lua, this: Table| with_editbox(lua, &this, |eb| eb.text.clone()))?,
    )?;
    // SetNumber(v) — **the same function as SetText**. `0x798690` and `0x7984c0` are byte-identical
    // (245 bytes each, zero differences after masking rel32 and absolute-VA operands; only the
    // usage string differs), so this does no numeric work of its own: it hands the argument to the
    // shared `lua_tostring` marshalling and sets the result as text.
    //
    // The GATE is `lua_isstring 0x6f3510`, a pure type test over {number, string} — NOT
    // `lua_isnumber` and NOT `luaL_checknumber`. So a STRING is accepted and passed through
    // VERBATIM, never parsed: `SetNumber("abc")` sets the text "abc" and does not raise. Anything
    // else — nil, boolean, table, or an ABSENT argument — raises the usage string and abandons the
    // caller's statement.
    //
    // The live consumer is `MoneyInputFrame.lua:47/52/57`, on the chain since 1751, whose three
    // boxes are `numeric="true"`. Our numeric filter already models the reference's wholesale
    // abort, which matters here: on such a box `SetNumber(-5)` or `SetNumber(0.8)` leaves the box
    // EMPTY rather than partially filled, because the sign or the point fails the digit test after
    // the clear-all has already run. The money path only ever passes non-negative integers
    // (`floor`/`mod` results), so it never takes that branch.
    m.set(
        "SetNumber",
        lua.create_function(|lua, (this, v): (Table, Value)| {
            let text = match &v {
                Value::Integer(i) => crate::script::binding_abi::lua_number_text(*i as f64),
                Value::Number(n) => crate::script::binding_abi::lua_number_text(*n),
                // A string is not parsed — it is the text.
                Value::String(s) => s.to_str()?.to_string(),
                _ => return Err(mlua::Error::runtime("Usage: EditBox:SetNumber(number)")),
            };
            let h = frame_handle_of(lua, &this)?;
            set_text(lua, h, &text);
            Ok(())
        })?,
    )?;
    // GetNumber: atof of the real text (0 on failure), matching `0x798790`.
    m.set(
        "GetNumber",
        lua.create_function(|lua, this: Table| {
            let text = with_editbox(lua, &this, |eb| eb.text.clone())?;
            Ok(text.trim().parse::<f64>().unwrap_or(0.0))
        })?,
    )?;
    // Insert(text) — **a nil is a no-op, not an error.** The reference's C `Insert` reads its
    // argument through `lua_tostring`, which answers NULL for a nil and leaves the buffer alone;
    // stock FrameXML relies on that. `LootFrame.lua:152` is the plain case:
    //
    //     ChatFrameEditBox:Insert(GetLootSlotLink(this.slot));
    //
    // with no guard, on a coin row whose `GetLootSlotLink` is nil. Typed as `String`, this raised
    // — which the loot window only survived while we owned the file and could add a guard the
    // reference does not have. It bit the moment `LootFrame.xml` came off the player's chain
    // (1751), and it would bite any addon writing the same unguarded line.
    //
    // A NUMBER still inserts its digits: `lua_tostring` converts one in place, and mlua's
    // `Option<String>` coercion follows it.
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
    // **No `HasFocus`.** 1.12's EditBox table registers the two setters above and no getter at
    // all, and an addon that wants the answer keeps its own flag around `SetFocus`/`ClearFocus` —
    // `pfQuest/browser.lua:760` documents exactly that and ships the workaround. Ours was an
    // unexplained superset a feature-detecting addon would take the wrong branch on (1188, and
    // the census that removed it, 2142). Host-side, the focus cell reads back through
    // [`crate::script::UiScript::focused_editbox_name`].

    // GetInputLanguage() / ToggleInputLanguage() — `0x799550` / `0x799610`, the edit box's
    // **IME** language, not the chat language: `ChatEdit_OnInputLanguageChanged` shows
    // `INPUT_<name>` on the box's language button, and the toggle is what a Korean client
    // flips between its two input modes. On a client with no IME the answer is the Roman
    // alphabet and the toggle moves nothing; benilla has no IME.
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

    // HighlightText([start [, end]]) — defaults (0, -1) = select-all.
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
        // `GetMaxLetters 0x79929f` — the read half, which we were missing. Not published off the
        // `strings` hit alone (that is what put `SetUnit` on the wrong widget earlier today):
        // `0x79929f` is the GETTER of `[widget+0x340]`, the same field the setter below writes,
        // which is table-level evidence rather than a name that happens to be in the image.
        //
        "GetMaxLetters",
        lua.create_function(|lua, this: Table| {
            with_editbox(lua, &this, |eb| eb.max_letters as i64)
        })?,
    )?;
    m.set(
        // `SetMaxBytes 0x798f30` — `SetMaxLetters`'s sibling in every respect but the sentinel:
        // the same EXACT count gate (`0x798fbc cmp eax,2`, the second of the four `lua_gettop`
        // callers), the same raw coerce of the value, and a field of its own at `+0x33c` whose
        // no-limit value is **-1** where `maxLetters`'s is 0 (`0x799012 jle` → `0x799022`; the
        // ctor writes -1 at `0x7799df`). So a non-positive argument is unlimited, a positive one
        // caps the buffer's BYTES. The stock StaticPopup_Show calls it for any entry carrying
        // `maxBytes` — none of the reference's own 76 does, so the reach is an addon's (1960).
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
        // The read half: -1 while unlimited, the reference's own stored sentinel.
        "GetMaxBytes",
        lua.create_function(|lua, this: Table| {
            with_editbox(lua, &this, |eb| eb.max_bytes.map_or(-1, |n| n as i64))
        })?,
    )?;
    m.set(
        // `SetMaxLetters 0x799110` — one of only FOUR widget bindings in the whole registrar that
        // calls `lua_gettop` (`0x6f3070`), and its gate is EXACT: `cmp eax,2` (`0x79919c`). So the
        // count is checked and the type is not, which is the opposite of the usual pairing and the
        // reason this needs its own body:
        //
        //   SetMaxLetters()           -> RAISES `Usage:` (too few)
        //   SetMaxLetters(50, 60)     -> RAISES `Usage:` (too MANY — an exact gate, not a minimum)
        //   SetMaxLetters(nil)        -> completes, stores 0
        //   SetMaxLetters("12")       -> completes, stores 12 (a numeric string coerces)
        //   SetMaxLetters({})         -> completes, stores 0
        //
        // and **0 is "no limit"**, not "no letters": the insert path's trim block is skipped
        // whole on zero (`0x77c085 test edi,edi; je`), which is what `max_letters == 0` already
        // means here. So `SetMaxLetters(nil)` is `SetMaxLetters(0)` is unlimited — aux-addon's
        // `gui/core.lua:288` writes exactly that, and benilla raised on it and killed the addon at
        // load. (Its neighbour `SetMaxBytes` uses **-1** for the same idea; two adjacent fields,
        // two different sentinels.)
        "SetMaxLetters",
        lua.create_function(|lua, (this, args): (Table, mlua::MultiValue)| {
            let args: Vec<Value> = args.into_iter().collect();
            if args.len() != 1 {
                return Err(mlua::Error::runtime(
                    "Usage: <unnamed>:SetMaxLetters(maxLetters)",
                ));
            }
            let n = crate::script::binding_abi::coerced_number(lua, args.first().cloned());
            // `__ftol` truncates toward zero; a negative stores as itself there, but our field is
            // a `usize` and the trim only ever tests `> 0`, so the two agree on every value that
            // can change behaviour.
            with_editbox(lua, &this, |eb| eb.max_letters = (n as i64).max(0) as usize)
        })?,
    )?;
    // The submitted-line history (`historyLines`): FrameXML pushes each sent line
    // (ChatEdit_AddHistory), UP/DOWN recall it (see `key_input`).
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
    // SetBlinkSpeed/GetBlinkSpeed — the caret half-period (`E+0x370`, XML `blinkSpeed`; ctor 0.5).
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
    // GetNumLetters: the LETTER count — `0x7992c0` walks the class array (`0x77bc80`), which
    // counts classes 2, 3 and 6 only, so escapes are free and a 43-byte item link reports 9.
    // Not bytes, and not chars either.
    m.set(
        "GetNumLetters",
        lua.create_function(|lua, this: Table| {
            with_editbox(lua, &this, |eb| {
                crate::markup::ClassMap::new(&eb.text).num_letters() as i64
            })
        })?,
    )?;

    // The config flags. All four mirror the live API. The fifth flag — bit4, the XML's
    // `ignoreArrows` — is NOT here: its Lua surface is `SetAltArrowKeyMode`/`GetAltArrowKeyMode`
    // below, which take a different argument coercion and answer a number rather than a boolean,
    // so it cannot share this loop. benilla used to register a `SetIgnoreArrows` "convenience"
    // for the loader to drive; 5875 has no such method (the 48-entry table
    // `[0x87bb68, 0x87bce8)` carries no entry whose name contains "Ignore"), so publishing it was
    // exactly the error decision 1189 names — and it stood in for the two real names, which were
    // missing. The loader drives the XML attribute through `SetAltArrowKeyMode` now.
    // ── SetAltArrowKeyMode / GetAltArrowKeyMode (`0x7996e0` / `0x799790`) ────────────────────
    //
    // **The setter's argument is `GetBoolOrDefault(L, 2, default = 1)`** (`0x6f1c10`), not Lua
    // truthiness and not a plain numeric coercion — `0x7996e0` pushes the default `1` before the
    // call, so **an absent argument ENABLES** the mode. `nil` disables; a number goes through
    // `__ftol` so `0` and `0.5` are false and `-1` is true; `""` matches nothing in the
    // off/disabled/on/enabled chain and falls to the default, so it ENABLES; `"0"` disables.
    // [`crate::script::binding_abi::bool_or_default`] already models every arm.
    //
    // **The getter answers the NUMBER 1 or nil**, never a boolean (`0x799815`: the set arm pushes
    // the double 1.0 via `0x6f3810`, the clear arm pushes nil via `0x6f37f0`) — the same idiom as
    // `IsShiftKeyDown`. An addon writing `if box:GetAltArrowKeyMode() then` reads either the same;
    // one writing `== 1` only reads the number.
    m.set(
        "SetAltArrowKeyMode",
        lua.create_function(|lua, (this, args): (Table, mlua::MultiValue)| {
            // `MultiValue`, not `Value`, because the reference DISTINGUISHES absent from nil here
            // and mlua's `Value` cannot: `0x7996e0` pushes the default `1` before the call, so
            // `SetAltArrowKeyMode()` enables while `SetAltArrowKeyMode(nil)` disables.
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
                    // The text-anchoring law reads multiLine (TOP vs MIDDLE), and the loader
                    // wires the declared `<FontString>` BEFORE the editbox flags (LoadXML order
                    // 5·b vs 5b) — re-seat an already-wired region so `multiLine="true"` lands.
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

/// **The font block — entries #0–#15 of the EditBox method table**, and the largest single gap the
/// per-kind widget-method census found in the 218-addon corpus.
///
/// `EditBox` is one of the six text-bearing types, so it re-declares the whole font block in its own
/// flat table (`.data 0x87bb68`, 48 entries, count read from `mov edx,0x30` at `0x799ab5`; there is
/// no `FontInstance` class in the 1.12 Lua chain). We had written every one of these verbs already —
/// on `FontString` and on the `<Font>` object — and never wired them to the kind that wanted them.
/// The census row is `63 EditBox:SetFontObject (on Texture, FontString)`, and its `(on …)` tail is
/// exactly that: *a verb we have, on the wrong kinds*.
///
/// **The 63 is one library, not 63 independent addons.** Every one of those call sites is the same
/// three lines of an embedded `Dewdrop-2.0.lua` —
/// ```lua
/// local editBox = CreateFrame("EditBox", nil, editBoxFrame)
/// editBoxFrame.editBox = editBox
/// editBox:SetFontObject(ChatFontNormal)
/// ```
/// — vendored into 63 addon folders (65 copies of the file: `FuBar` and its ~50 plugins, `BigWigs`,
/// `AtlasLoot`, `oRA2`, …). One library replicated, so 63 chances to hit the *same* next wall.
///
/// Ten of the sixteen are the shared block and come from [`super::super::font_block`], which carries
/// the per-verb byte evidence and the return-shape traps. Two are deliberately absent and four are
/// installed here:
///
/// - **`SetSpacing`/`GetSpacing` (#10–#11) are NOT installed.** Nothing in this client models line
///   spacing, so they could only store a number no renderer reads — the silently-ignored-setter
///   failure of 1203/1205/1211 — while their absence raises a nil-value call that names itself.
///   Corpus demand is zero: `:SetSpacing(`/`:GetSpacing(` appear in **0** of 218 addons.
///   `script::font` withholds the same pair for the same reason.
/// - **The justify four (#12–#15) are installed here**, against the box's own
///   [`EditBoxState::justify`] word rather than its text region — the field's doc has the why (our
///   editbox draw law seats that region LEFT unconditionally) and states the divergence.
fn install_font_block(lua: &Lua, m: &Table) -> mlua::Result<()> {
    // Every font method acts on the box's implicit FontString: each binding's shim loads
    // `[this+0x324]` and hands it to the shared implementation, and that offset is
    // `EditBoxState::text_region`. Created on demand, exactly like every other text-touching path.
    crate::script::font_block::install(
        lua,
        m,
        |lua, this| {
            let h = frame_handle_of(lua, this)?;
            super::ensure_text_region(lua, h).ok_or_else(|| mlua::Error::runtime("not an EditBox"))
        },
        "EditBox",
    )?;

    // SetJustifyH("LEFT"|"CENTER"|"RIGHT") / SetJustifyV("TOP"|"MIDDLE"|"BOTTOM") → 0 values.
    //
    // Two verified traps, both of which a plausible implementation gets wrong. This table was the
    // only one that got them right; the FontString and `<Font>` copies coerced anything unknown to
    // CENTER/MIDDLE until 1237 lifted this law into [`crate::justify`], which all three now share:
    //  · an **unrecognised token RAISES** `Usage: %s:SetJustifyH("justify")` (`0x87c77c`), rather
    //    than falling back to a default;
    //  · a token from the **other axis** parses fine and then masks to nothing — `SetJustifyH("TOP")`
    //    yields 0x08, `0x08 & 0x07 == 0`, so it CLEARS justifyH and `GetJustifyH()` answers
    //    `"UNKNOWN"`. No error either way.
    //
    // `AceGUIWidget-Slider.lua:210` (`editbox:SetJustifyH("CENTER")`, three addons) is the demand.
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
    // GetJustifyH()/GetJustifyV() → exactly 1 string, the first set bit's token in the reference's
    // own table order, or the literal "UNKNOWN".
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

/// A `Set<Flag>` name paired with the EditBoxState mutator it drives (Lua truthiness).
type FlagSetter = (&'static str, fn(&mut EditBoxState, bool));

/// The config-flag setters, in one place so the loader and the Lua surface share the list.
fn flag_setters() -> [FlagSetter; 4] {
    [
        ("SetAutoFocus", |eb, on| eb.auto_focus = on),
        ("SetNumeric", |eb, on| eb.numeric = on),
        ("SetPassword", |eb, on| eb.password = on),
        ("SetMultiLine", |eb, on| eb.multi_line = on),
    ]
}
