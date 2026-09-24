//! The `MessageFrame` method surface — `CSimpleMessageFrame` (ctor `0x785640`), the class
//! `UIErrorsFrame` is and the one `CreateFrame("MessageFrame")` makes.
//!
//! Three facts decide this whole file, and each is one an implementation would otherwise get wrong
//! in a way nothing would catch:
//!
//! 1. **`AddMessage(text [, r, g, b [, a]])` — the fifth argument is ALPHA and there is no sixth.**
//!    Three corpus addons pass six (`EasyCopy.lua:12`, `QuestHistory.lua:1678`, `QuestItem.lua:300`,
//!    all `UIErrorsFrame:AddMessage(msg, r, g, b, a, holdTime)`), and the real 1.12.1 binding
//!    `0x795590` reads five and stops. Honouring that trailing number as a per-message hold time
//!    would be **unfaithful, not generous** — the client shows the message for the *frame's*
//!    `displayDuration` and nothing else. It is ignored here, deliberately.
//! 2. **This `AddMessage` is not the scrolling class's.** `0x792900` takes an **id** in the same
//!    slot and forces alpha `0xFF` (`792add: or edi,0xffffff00`); `0x795590` packs a real
//!    `0xAARRGGBB` with a default of 1.0. Two bindings, two tables, no sharing.
//! 3. **`SetInsertMode`/`GetInsertMode` (`0x794ed0`/`0x794ff0`) live on this class only** — the
//!    scrolling class has neither, and no `insertMode` XML attribute either.
//!
//! The state (display lines, fade phases, the vertical cap) is
//! [`MessageFrameState`](crate::widget::MessageFrameState) in `crate::widget`; this is the thin Lua
//! binding over it. The band emit and the wrapped-row measure round-trip are shared with the
//! scrolling class and live in [`super`].

use mlua::{Lua, Table, Value};

use crate::script::object::frame_handle_of;
use crate::script::Model;
use crate::widget::{InsertMode, KindState, MessageFrameState};

/// Registry key of the MessageFrame method table (the MAXCSTACK discipline: Lua-side root, named
/// key).
pub(crate) const REG_MESSAGEFRAME_METHODS: &str = "__benilla_plain_messageframe_methods";

/// Run `f` over a frame's MessageFrame state under one short write borrow. Errors if `this` is not
/// a live MessageFrame (unreachable through the kind dispatcher, but the method table is a plain
/// Lua value — a caller could fish it out and misapply it).
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

/// A Lua number-ish → f32 (nil/other → 0.0), for the colour args.
fn num_f32(v: &Value) -> f32 {
    match v {
        Value::Number(n) => *n as f32,
        Value::Integer(i) => *i as f32,
        _ => 0.0,
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;

    // The justify quartet, shared with the sibling class — see [`super::install_justify`] for the
    // method-table bytes that put all four on BOTH tables.
    super::install_justify(lua, &m, "MessageFrame")?;

    // AddMessage(text [, r, g, b [, a]]) — binding 0x795590.
    //
    // The TEXT is `super::message_text` — an `lua_isstring` gate whose failure is a silent jump to
    // this function's own epilogue, not a raise, and the same for a NULL coercion and for the
    // empty string. Its doc carries the bytes and the reason it matters; the short version is that
    // stock `ContainerFrame.lua` really does reach `AddMessage(nil, …)` on a full keyring, and
    // taking a `String` here (which this did until 1751's fourth window) put a script-error dialog
    // on a gesture the reference answers with a dead click.
    //
    // r/g/b are required **as a trio** (three presence checks ANDed at 0x7956xx); absent ⇒ hard
    // 0xFFFFFFFF white. The fourth numeric is a real alpha, `lua_isnumber`-gated with a default of
    // 1.0 (`795752: mov [ebp-0x18],0x3f800000`). **Deliberately NOT the same shape as the text:** a
    // bad colour jumps to `0x79581c`, which still enqueues the line in opaque white. Only a bad
    // text shows nothing.
    //
    // **The signature stops there, and the closure's arity is the enforcement.** The corpus's three
    // blocked callers all pass a sixth argument they believe is a hold time; mlua hands us the
    // first five and drops the rest, exactly as the real binding does by reading five stack slots.
    // Adding a `holdTime` parameter here would make benilla show those messages for a duration no
    // 1.12 client ever showed them for.
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
                // Alpha only counts when rgb came with it — the client reads arg 5 off the same
                // parse that required the trio, and a lone `AddMessage(text, nil, nil, nil, 0.5)`
                // has no colour to apply it to.
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

    // SetInsertMode("TOP"|"BOTTOM") / GetInsertMode — 0x794ed0 / 0x794ff0, THIS CLASS ONLY. The
    // client compares the string against "BOTTOM" (0x871404) and stores `streq-BOTTOM`, so anything
    // that is not literally BOTTOM lands on TOP; the ctor default is 1 = BOTTOM.
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
                // A non-string argument fails the streq and therefore reads as TOP, like the
                // client's own comparison against a string it never got.
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

    // The fade trio — the same names and the same field the scrolling class exposes (the XML attr
    // is `displayDuration`, the Lua accessors call it `TimeVisible`), over this class's own state.
    m.set(
        "SetFading",
        lua.create_function(|lua, (this, on): (Table, Value)| {
            let on = !matches!(on, Value::Nil | Value::Boolean(false));
            with_mf(lua, &this, |mf| mf.fading_enabled = on)
        })?,
    )?;
    // 1/nil, the reference's predicate shape (`0x795170`): `(nil) | (number)`, like every other
    // 1.12 predicate (decision 2118).
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
    //
    // `Set/GetFontObject · Set/GetFont · Set/GetTextColor · Set/GetShadowColor ·
    // Set/GetShadowOffset` are real entries on this class's table, not a courtesy. The
    // registrar map is explicit about the membership — *"Exposed on: FontString, Font object,
    // EditBox, MessageFrame, ScrollingMessageFrame, SimpleHTML. NOT on Button"* (table
    // `0x879d00` has none) — and names this class's own shims calling the shared
    // implementations (`GetShadowColor 0x794810`). All six carry the block now —
    // `SimpleHTML`'s is its own copy rather than `font_block::install`'s
    // (`script/simplehtml/mod.rs`), because that class computes its own inter-block step.
    //
    // Demand is observed, not counted: `BigWigs/Plugins/Messages.lua:212` is
    // `self.msgframe:SetFontObject(GameFontNormalLarge)` on a frame it has just given
    // `SetInsertMode("TOP")`, and BigWigs dies there every session.
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

    /// How many display lines a named message frame holds. Read off the state rather than through
    /// Lua on purpose: `GetNumMessages` is a **ScrollingMessageFrame** binding, and inventing one
    /// on this class to make its tests convenient is exactly the drift these tests are pinning
    /// against.
    fn num_messages(s: &UiScript, name: &str) -> usize {
        let m = s.model_ref();
        let h = m.arena.lookup(name).expect("named frame");
        m.arena
            .frame(h)
            .and_then(|f| f.kind_state.message_lines())
            .map_or(0, |l| l.len())
    }

    /// The whole point of the arity, pinned: the sixth argument three corpus addons pass is a hold
    /// time the real client never had, and it must change **nothing**. The frame's own
    /// `displayDuration` is what governs, so the message with a "hold time" of 999 fades on exactly
    /// the same tick as the one without.
    #[test]
    fn the_sixth_addmessage_argument_is_ignored_not_a_holdtime() {
        let mut s = UiScript::new().unwrap();
        s.run("CreateFrame('MessageFrame', 'MF')").unwrap();
        s.run("MF:SetTimeVisible(1)").unwrap();
        s.run("MF:SetFadeDuration(0)").unwrap();
        // The exact shape of EasyCopy.lua:12 / QuestItem.lua:300 / QuestHistory.lua:1678.
        s.run("MF:AddMessage('held', 1.0, 1.0, 1.0, 1.0, 999)")
            .unwrap();
        s.run("MF:AddMessage('plain', 1.0, 1.0, 1.0, 1.0)").unwrap();
        assert_eq!(num_messages(&s, "MF"), 2);
        // Past timeVisible with no ramp: both retire together. A honoured holdTime would keep the
        // first alive here, which is exactly the divergence this test exists to catch.
        s.tick(1.1);
        s.tick(0.1);
        assert_eq!(
            num_messages(&s, "MF"),
            0,
            "the 6th arg must not extend a message's life"
        );
    }

    /// **A text `AddMessage` cannot use is swallowed whole — silently, with nothing added and
    /// nothing raised** (`0x795590`'s three jumps to its own epilogue; see [`super::super`]'s
    /// `message_text`).
    ///
    /// The defect this pins is not hypothetical and not ours to have guessed at: our binding took
    /// its text as an mlua `String`, which raises on nil, and 1.12's own `ContainerFrame.lua:753`
    /// reports a full keyring through `UIErrorsFrame:AddMessage(NO_EMPTY_KEYRING_SLOTS, …)` — a
    /// GlobalString the shipped `GlobalStrings.lua` never defines. So dropping a key onto a full
    /// keyring put a script-error dialog on screen where the reference gives a dead click. It
    /// surfaced the moment 1751's third window made the bag bar the reference's own file.
    ///
    /// The three silent cases and the two that are NOT silent are pinned together, because the
    /// pair is the finding: a bad **colour** still adds the line (opaque white), and the receiver
    /// guards still raise. Reading the text gate as a general argument law is exactly 1717's error.
    #[test]
    fn addmessage_swallows_a_text_it_cannot_use_and_only_the_text() {
        let s = UiScript::new().unwrap();
        s.run("CreateFrame('MessageFrame', 'MF')").unwrap();

        // `lua_isstring 0x6f3510` accepts tags 4 and 3 only — everything else jumps to 0x79582b.
        for bad in ["nil", "", "true", "{}", "print"] {
            s.run(&format!("MF:AddMessage({bad})"))
                .unwrap_or_else(|e| panic!("AddMessage({bad}) must not raise: {e}"));
        }
        // …and so does the empty string, on its own `cmp byte ptr [ebx],0` at 0x79564b.
        s.run("MF:AddMessage('')").unwrap();
        assert_eq!(
            num_messages(&s, "MF"),
            0,
            "nil, absent, boolean, table, function and \"\" each add NOTHING"
        );

        // A number IS a usable text — tag 3 passes the gate and `0x6f7c80` retags the slot in
        // place, so it renders as its decimal form.
        s.run("MF:AddMessage(42)").unwrap();
        assert_eq!(num_messages(&s, "MF"), 1, "a number is a text");

        // The colour arguments are the OPPOSITE shape and that is the load-bearing negative: a bad
        // r/g/b jumps to 0x79581c, which still enqueues with the 0xffffffff white staged at
        // 0x795658. Suppressing the line here would be the same over-generalisation in reverse.
        s.run("MF:AddMessage('kept', {}, nil, 'x')").unwrap();
        assert_eq!(
            num_messages(&s, "MF"),
            2,
            "a bad colour still adds the line"
        );

        // The receiver guards are upstream of all of it and DO raise (0x847ef8 — "used '.' instead
        // of ':'").
        assert!(
            s.run("MF.AddMessage('dot')").is_err(),
            "a '.'-instead-of-':' call still raises"
        );
    }

    /// `SetInsertMode` is a MessageFrame method and **only** a MessageFrame method — a plain Frame
    /// and a ScrollingMessageFrame both resolve it to nil, which is what a duck-typing addon reads.
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
        // Default BOTTOM (ctor 1), and the round trip.
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

    /// The two `AddMessage`s are different functions, and the observable tell is the fifth argument:
    /// on a MessageFrame it is alpha (0.5 ⇒ a half-transparent line), on a ScrollingMessageFrame it
    /// is an id and the line stays opaque.
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

    /// `insertMode` decides which edge the stack grows from: TOP hangs the newest message off the
    /// frame's top, BOTTOM (the default) stacks up from its bottom. Same three messages, mirrored.
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

    /// The class has no `maxLines` — its cap is what fits vertically, applied at the tick. A frame
    /// 3 rows tall holds 3 messages however many arrive.
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
        // ...and it is the OLDEST that went.
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

    /// A faded line on this class is **freed**, not left blank holding its rows — the difference
    /// from the scrolling class, whose ring slots persist. `Clear` is the immediate form.
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

    /// The ctor defaults (`0x81cc2c`/`0x81cc30`: timeVisible, fadeDuration), plus the setters'
    /// round trip.
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
