//! The `ScrollingMessageFrame` methods over [`ScrollingMessageState`]: `CSimpleMessageScrollFrame`
//! (ctor `0x787670`), the chat window's class. A ring of `maxLines` that drops the oldest, colours
//! quantized `trunc(x*255+0.5)` with alpha forced opaque, a fade that ticks only at the bottom, and
//! every scroll call re-arming the displayed lines' fade (`0x788b80` when refused at an end,
//! `0x788af0` when it moves), which is what brings a faded chat back.

use mlua::{Lua, Table, Value};

use crate::script::object::frame_handle_of;
use crate::script::{Model, UiScript};
use crate::widget::{KindState, ScrollingMessageState};

/// Registry key of the ScrollingMessageFrame method table (the MAXCSTACK discipline).
pub(crate) const REG_SCROLLINGMESSAGEFRAME_METHODS: &str =
    "__benilla_scrollingmessageframe_methods";

/// Run `f` over a ScrollingMessageFrame's state under one short borrow; any other frame, reachable
/// only through a misapplied method table, is an error.
fn with_smf<T>(
    lua: &Lua,
    this: &Table,
    f: impl FnOnce(&mut ScrollingMessageState) -> T,
) -> mlua::Result<T> {
    let h = frame_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let frame = model
        .arena
        .frame_mut(h)
        .ok_or_else(|| mlua::Error::runtime("stale frame handle"))?;
    match &mut frame.kind_state {
        KindState::ScrollingMessage(smf) => Ok(f(smf)),
        _ => Err(mlua::Error::runtime("not a ScrollingMessageFrame")),
    }
}

/// `lua_isnumber`: a number or a numeric string. The rgb presence test is three of these.
fn is_number(v: &Value) -> bool {
    match v {
        Value::Integer(_) | Value::Number(_) => true,
        Value::String(s) => s.to_str().is_ok_and(|s| s.trim().parse::<f64>().is_ok()),
        _ => false,
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

/// Run a scroll call with the viewport's row count, which every scroll needs: each re-arms the
/// fade of the lines on display ([`ScrollingMessageState::reset_all_fade_times`]).
fn scroll(
    lua: &Lua,
    this: &Table,
    op: impl FnOnce(&mut ScrollingMessageState, usize),
) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let viewport_rows = UiScript::message_viewport_rows(&model, h);
    let frame = model
        .arena
        .frame_mut(h)
        .ok_or_else(|| mlua::Error::runtime("stale frame handle"))?;
    match &mut frame.kind_state {
        KindState::ScrollingMessage(smf) => {
            op(smf, viewport_rows);
            Ok(())
        }
        _ => Err(mlua::Error::runtime("not a ScrollingMessageFrame")),
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;

    // The justify quartet, on both classes ([`super::install_justify`]).
    super::install_justify(lua, &m, "ScrollingMessageFrame")?;

    // AddMessage(text [, r, g, b [, id]]) (`0x792900`): white without rgb; the state quantizes
    // the colour and makes the line opaque. The rgb test is three `lua_isnumber` checks; the id
    // is stack index 6 when they pass (`0x792b13`) and index 3, `r`'s, when they fail
    // (`0x792b48`), which makes `AddMessage(text, id)` work and gives
    // `AddMessage(text, nil, nil, nil, nil, 5)` id 0. A missing id is 0, which `UpdateColorByID`
    // never matches (`0x788250`). The text gate (`0x79299c`) is the plain class's
    // ([`super::message_text`]).
    m.set(
        "AddMessage",
        lua.create_function(
            |lua, (this, text, r, g, b, id): (Table, Value, Value, Value, Value, Value)| {
                let Some(text) = super::message_text(lua, &text) else {
                    return Ok(());
                };
                let has_rgb = is_number(&r) && is_number(&g) && is_number(&b);
                // The chat-type index (`ChatFrame.lua` passes `info.id`), stack index 6 or 3.
                let id = match if has_rgb { &id } else { &r } {
                    Value::Integer(i) => u32::try_from(*i).unwrap_or(0),
                    Value::Number(n) if n.is_finite() && *n >= 0.0 => *n as u32,
                    _ => 0,
                };
                let (r, g, b) = if has_rgb {
                    (num_f32(&r), num_f32(&g), num_f32(&b))
                } else {
                    (1.0, 1.0, 1.0)
                };
                with_smf(lua, &this, |smf| smf.add_with_id(text, r, g, b, id))
            },
        )?,
    )?;

    // UpdateColorByID(id, r, g, b) (`0x7932b0`) recolours the lines printed under `id`; stock
    // `ChatFrame_OnEvent` calls it on `UPDATE_CHAT_COLOR`.
    m.set(
        "UpdateColorByID",
        lua.create_function(
            |lua, (this, id, r, g, b): (Table, Value, Value, Value, Value)| {
                let id = match &id {
                    Value::Integer(i) => u32::try_from(*i).unwrap_or(0),
                    Value::Number(n) if n.is_finite() && *n >= 0.0 => *n as u32,
                    _ => return Ok(()),
                };
                let (r, g, b) = (num_f32(&r), num_f32(&g), num_f32(&b));
                with_smf(lua, &this, |smf| {
                    smf.update_color_by_id(id, r, g, b);
                })
            },
        )?,
    )?;

    m.set(
        "Clear",
        lua.create_function(|lua, this: Table| with_smf(lua, &this, ScrollingMessageState::clear))?,
    )?;
    m.set(
        "ScrollUp",
        lua.create_function(|lua, this: Table| {
            scroll(lua, &this, ScrollingMessageState::scroll_up)
        })?,
    )?;
    m.set(
        "ScrollDown",
        lua.create_function(|lua, this: Table| {
            scroll(lua, &this, ScrollingMessageState::scroll_down)
        })?,
    )?;
    m.set(
        "ScrollToTop",
        lua.create_function(|lua, this: Table| {
            scroll(lua, &this, ScrollingMessageState::scroll_to_top)
        })?,
    )?;
    m.set(
        "ScrollToBottom",
        lua.create_function(|lua, this: Table| {
            scroll(lua, &this, ScrollingMessageState::scroll_to_bottom)
        })?,
    )?;
    m.set(
        "PageUp",
        lua.create_function(|lua, this: Table| scroll(lua, &this, ScrollingMessageState::page_up))?,
    )?;
    m.set(
        "PageDown",
        lua.create_function(|lua, this: Table| {
            scroll(lua, &this, ScrollingMessageState::page_down)
        })?,
    )?;
    m.set(
        "AtTop",
        lua.create_function(|lua, this: Table| with_smf(lua, &this, |smf| smf.at_top()))?,
    )?;
    m.set(
        "AtBottom",
        lua.create_function(|lua, this: Table| with_smf(lua, &this, |smf| smf.at_bottom()))?,
    )?;

    // SetMaxLines wipes the ring (`0x7938a0`).
    m.set(
        "SetMaxLines",
        lua.create_function(|lua, (this, n): (Table, i64)| {
            with_smf(lua, &this, |smf| smf.set_max_lines(n.max(1) as usize))
        })?,
    )?;
    m.set(
        "GetMaxLines",
        lua.create_function(|lua, this: Table| with_smf(lua, &this, |smf| smf.max_lines as i64))?,
    )?;
    m.set(
        "GetNumMessages",
        lua.create_function(|lua, this: Table| with_smf(lua, &this, |smf| smf.lines.len() as i64))?,
    )?;

    m.set(
        "SetFading",
        lua.create_function(|lua, (this, on): (Table, Value)| {
            let on = !matches!(on, Value::Nil | Value::Boolean(false));
            with_smf(lua, &this, |smf| smf.fading_enabled = on)
        })?,
    )?;
    // 1 or nil, the 1.12 predicate shape (`0x793a40`).
    m.set(
        "GetFading",
        lua.create_function(|lua, this: Table| {
            with_smf(lua, &this, |smf| {
                crate::script::binding_abi::flag(smf.fading_enabled)
            })
        })?,
    )?;
    // XML's `displayDuration` is the Lua `TimeVisible` (`0x788090`).
    m.set(
        "SetTimeVisible",
        lua.create_function(|lua, (this, s): (Table, f32)| {
            with_smf(lua, &this, |smf| smf.time_visible = s.max(0.0))
        })?,
    )?;
    m.set(
        "GetTimeVisible",
        lua.create_function(|lua, this: Table| with_smf(lua, &this, |smf| smf.time_visible))?,
    )?;
    m.set(
        "SetFadeDuration",
        lua.create_function(|lua, (this, s): (Table, f32)| {
            with_smf(lua, &this, |smf| smf.fade_duration = s.max(0.0))
        })?,
    )?;
    m.set(
        "GetFadeDuration",
        lua.create_function(|lua, this: Table| with_smf(lua, &this, |smf| smf.fade_duration))?,
    )?;

    // ── the shared font block ───────────────────────────────────────────────────────────────
    // The ten font verbs are on this class's own table (`GetShadowColor` `0x792240`), as on
    // FontString, Font, EditBox, MessageFrame and SimpleHTML; Button's (`0x879d00`) has none.
    crate::script::font_block::install(
        lua,
        &m,
        |lua, this| {
            let h = frame_handle_of(lua, this)?;
            super::ensure_font_region(lua, h)
                .ok_or_else(|| mlua::Error::runtime("not a ScrollingMessageFrame"))
        },
        "ScrollingMessageFrame",
    )?;

    lua.set_named_registry_value(REG_SCROLLINGMESSAGEFRAME_METHODS, m)?;

    // BenillaChatTabPressed(): the chat edit box's OnTabPressed sets a flag for the app's whisper
    // cycle (`UiScript::take_chat_tab`).
    lua.globals().set(
        "BenillaChatTabPressed",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.chat_tab = true;
            Ok(())
        })?,
    )?;
    // SubmitChatInput(text): the chat edit box's OnEnterPressed queues the line for the app to
    // parse as a chat command; an empty line is queued too, and the app takes it as a cancel.
    lua.globals().set(
        "SubmitChatInput",
        lua.create_function(|lua, text: Option<String>| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.chat_input.push(text.unwrap_or_default());
            Ok(())
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;
    use crate::widget::ScrollingMessageState;

    fn white(text: &str) -> (String, f32, f32, f32) {
        (text.to_string(), 1.0, 1.0, 1.0)
    }

    #[test]
    fn ring_drops_oldest_past_max_lines() {
        let mut s = ScrollingMessageState {
            max_lines: 3,
            ..Default::default()
        };
        for n in 0..5 {
            let (t, r, g, b) = white(&format!("line {n}"));
            s.add(t, r, g, b);
        }
        assert_eq!(s.lines.len(), 3);
        let texts: Vec<&str> = s.lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["line 2", "line 3", "line 4"]);
    }

    #[test]
    fn set_max_lines_is_destructive() {
        let mut s = ScrollingMessageState::default();
        for n in 0..4 {
            s.add(format!("m{n}"), 1.0, 1.0, 1.0);
        }
        assert_eq!(s.lines.len(), 4);
        s.set_max_lines(128);
        assert!(s.lines.is_empty(), "SetMaxLines wipes the ring");
        assert_eq!(s.max_lines, 128);
        assert!(s.at_bottom());
    }

    #[test]
    fn color_is_quantized_round_half_up() {
        let mut s = ScrollingMessageState::default();
        // trunc(0.5 * 255 + 0.5) = 128; 1.0 is 255, and past 1.0 clamps.
        s.add("q".into(), 0.5, 1.0, 2.0);
        assert_eq!(s.lines[0].color, [128, 255, 255]);
        // EMOTE's FF8040 survives the byte round trip.
        s.add("e".into(), 1.0, 128.0 / 255.0, 64.0 / 255.0);
        assert_eq!(s.lines[1].color, [255, 128, 64]);
    }

    #[test]
    fn fade_holds_then_ramps_then_retires() {
        let mut s = ScrollingMessageState {
            time_visible: 1.0,
            fade_duration: 2.0,
            ..Default::default()
        };
        s.add("x".into(), 1.0, 1.0, 1.0);
        // Phase 1: full alpha while timeVisible remains.
        s.tick(0.5);
        assert_eq!(s.lines[0].alpha, 1.0);
        // 0.5 more spends timeVisible; the ticks after ramp over fadeDuration.
        s.tick(0.5);
        assert_eq!(s.lines[0].alpha, 1.0, "phase 1 just expired, ramp not yet");
        s.tick(1.0); // fade_left 2.0 → 1.0 → alpha = trunc(1.0/2.0*255)/255 = 127/255
        assert!((s.lines[0].alpha - 127.0 / 255.0).abs() < 1e-6);
        s.tick(1.0); // fade_left → 0 → retired
        assert_eq!(s.lines[0].alpha, 0.0);
    }

    #[test]
    fn fade_duration_zero_vanishes_instantly() {
        let mut s = ScrollingMessageState {
            time_visible: 0.5,
            fade_duration: 0.0,
            ..Default::default()
        };
        s.add("x".into(), 1.0, 1.0, 1.0);
        s.tick(0.6); // spends timeVisible, still full this tick
        assert_eq!(s.lines[0].alpha, 1.0);
        s.tick(0.1); // past phase 1 with no ramp: straight to 0
        assert_eq!(s.lines[0].alpha, 0.0);
    }

    /// With a one-row viewport the scroll re-arms one line; the lines either side show the freeze.
    #[test]
    fn scrolled_up_freezes_the_fade() {
        let mut s = ScrollingMessageState {
            max_lines: 8,
            time_visible: 0.0, // straight into phase 2
            fade_duration: 4.0,
            ..Default::default()
        };
        for n in 0..3 {
            s.add(format!("l{n}"), 1.0, 1.0, 1.0);
        }
        s.tick(2.0); // half-way down the ramp: trunc(2.0/4.0*255) = 127
        let half = 127.0 / 255.0;
        assert_eq!(s.lines[0].alpha, half);
        // Scrolled up, ticks stop; the scroll re-arms only l1, the line now in view.
        s.scroll_up(1);
        assert!(!s.at_bottom());
        assert_eq!(s.lines[1].alpha, 1.0, "the displayed line was re-armed");
        s.tick(2.0);
        assert_eq!(s.lines[0].alpha, half, "frozen while scrolled up");
        assert_eq!(s.lines[2].alpha, half);
        assert_eq!(s.lines[1].alpha, 1.0);
        // Back at the bottom, the fade resumes.
        s.scroll_to_bottom(1);
        s.tick(2.0);
        assert!(s.lines[2].alpha < 1.0);
    }

    /// Every scroll call re-arms the displayed lines, even one that moves no cursor, as clicking
    /// the arrows on a chat already at the bottom does.
    #[test]
    fn any_scroll_brings_faded_lines_back() {
        let armed = |op: fn(&mut ScrollingMessageState, usize)| {
            let mut s = ScrollingMessageState {
                max_lines: 8,
                time_visible: 1.0,
                fade_duration: 1.0,
                ..Default::default()
            };
            for n in 0..3 {
                s.add(format!("l{n}"), 1.0, 1.0, 1.0);
            }
            s.tick(1.5); // spend phase 1
            s.tick(1.5); // spend phase 2: all gone
            assert!(s.lines.iter().all(|l| l.alpha == 0.0), "faded out first");
            op(&mut s, 8);
            s
        };
        for (name, op) in [
            (
                "ScrollUp",
                ScrollingMessageState::scroll_up as fn(&mut _, usize),
            ),
            ("ScrollDown", ScrollingMessageState::scroll_down),
            ("ScrollToTop", ScrollingMessageState::scroll_to_top),
            ("ScrollToBottom", ScrollingMessageState::scroll_to_bottom),
            ("PageUp", ScrollingMessageState::page_up),
            ("PageDown", ScrollingMessageState::page_down),
        ] {
            let s = armed(op);
            // The re-armed set is what the op left in view, not the whole ring.
            let shown = s.displayed_range(8);
            assert!(!shown.is_empty(), "{name} displayed nothing");
            for i in shown {
                let line = &s.lines[i];
                assert_eq!(line.alpha, 1.0, "{name} left line {i} invisible");
                assert_eq!(line.time_left, 1.0, "{name} left line {i} un-armed");
                assert_eq!(line.fade_left, 1.0, "{name} left line {i} un-armed");
            }
        }
    }

    /// `0x788b80` walks the lines on display, not the ring.
    #[test]
    fn the_re_arm_reaches_only_the_displayed_lines() {
        let mut s = ScrollingMessageState {
            max_lines: 8,
            time_visible: 1.0,
            fade_duration: 1.0,
            ..Default::default()
        };
        for n in 0..3 {
            s.add(format!("l{n}"), 1.0, 1.0, 1.0);
        }
        s.tick(1.5);
        s.tick(1.5);
        s.scroll_down(1); // a no-op scroll at the bottom, one row of viewport
        assert_eq!(s.lines[2].alpha, 1.0, "the one displayed line came back");
        assert_eq!(s.lines[1].alpha, 0.0, "off-screen lines are untouched");
        assert_eq!(s.lines[0].alpha, 0.0);
    }

    #[test]
    fn scroll_clamps_at_both_ends() {
        let mut s = ScrollingMessageState {
            max_lines: 8,
            ..Default::default()
        };
        for n in 0..3 {
            s.add(format!("l{n}"), 1.0, 1.0, 1.0);
        }
        assert!(s.at_bottom());
        s.scroll_down(8); // already at the bottom: no move
        assert!(s.at_bottom());
        // 3 lines → max_scroll = 2; scroll past it clamps.
        for _ in 0..10 {
            s.scroll_up(8);
        }
        assert!(s.at_top());
        assert_eq!(s.scroll_offset, 2);
        s.scroll_to_bottom(8);
        assert!(s.at_bottom());
    }

    #[test]
    fn scrolled_view_stays_anchored_as_the_ring_grows() {
        let mut s = ScrollingMessageState {
            max_lines: 8,
            ..Default::default()
        };
        for n in 0..4 {
            s.add(format!("l{n}"), 1.0, 1.0, 1.0);
        }
        s.scroll_up(8); // viewing one line back (bottom row = l2)
        let off = s.scroll_offset;
        s.add("l4".into(), 1.0, 1.0, 1.0);
        assert_eq!(
            s.scroll_offset,
            off + 1,
            "offset tracks so the same lines stay in view"
        );
    }

    #[test]
    fn displayed_count_and_paging_respect_wrapped_rows() {
        let mut s = ScrollingMessageState {
            max_lines: 8,
            ..Default::default()
        };
        for n in 0..3 {
            s.add(format!("l{n}"), 1.0, 1.0, 1.0);
        }
        s.lines[1].rows = 3;
        // 4 rows: the newest (1) and the middle (3) fill it; a partly fitting message counts.
        assert_eq!(s.displayed_count(4), 2);
        assert_eq!(s.displayed_count(3), 2, "partial middle line still counts");
        assert_eq!(s.displayed_count(1), 1);
        // A page is displayed − 1, one message of overlap, never 0.
        s.page_up(4);
        assert_eq!(s.scroll_offset, 1);
        s.page_up(1); // displayed 1 → page clamps to 1 message
        assert_eq!(s.scroll_offset, 2);
        s.page_down(4);
        assert_eq!(s.scroll_offset, 1);
        s.page_down(4);
        assert!(s.at_bottom());
    }

    #[test]
    fn emit_stacks_wrapped_rows_and_clips_partial_top() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        s.run(
            "local f = CreateFrame('ScrollingMessageFrame', 'CF')\n\
             f:SetPoint('BOTTOMLEFT', 32, 90)\n\
             f:SetWidth(430)\n\
             f:SetHeight(34)",
        )
        .unwrap();
        s.run("CF:AddMessage('old', 1, 1, 1)").unwrap();
        s.run("CF:AddMessage('wrapped', 1, 1, 1)").unwrap();
        s.run("CF:AddMessage('new', 1, 1, 1)").unwrap();
        s.resolve();
        let reqs = s.message_lines_needing_measure();
        assert_eq!(reqs.len(), 3);
        assert!(reqs.iter().all(|r| (r.wrap_width - 430.0).abs() < 0.5));
        let answers: Vec<(u32, u32, u16, u64)> = reqs
            .iter()
            .map(|r| (r.frame, r.index, if r.index == 1 { 2 } else { 1 }, r.key))
            .collect();
        s.set_message_line_rows(&answers);
        assert!(
            s.message_lines_needing_measure().is_empty(),
            "answered keys satisfy the cache"
        );
        // Default font, pitch 14. Frame [90, 124): 'new' is [90, 104), 'wrapped' [104, 132)
        // overflows and draws clipped, and 'old' would start at 132, outside, so does not draw.
        let quads = s.extract();
        let texts: Vec<(String, f32, f32)> = quads
            .iter()
            .filter_map(|q| match (&q.content, q.rect) {
                (crate::script::QuadContent::Text { text: Some(t), .. }, Some(r)) => {
                    Some((t.clone(), r.bottom, r.top))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            texts.len(),
            2,
            "partial top line draws, off-top line doesn't"
        );
        assert_eq!(texts[0].0, "new");
        assert!((texts[0].1 - 90.0).abs() < 0.01 && (texts[0].2 - 104.0).abs() < 0.01);
        assert_eq!(texts[1].0, "wrapped");
        assert!((texts[1].1 - 104.0).abs() < 0.01 && (texts[1].2 - 132.0).abs() < 0.01);
        let clip = quads
            .iter()
            .find_map(|q| match &q.content {
                crate::script::QuadContent::Text { text: Some(t), .. } if t == "wrapped" => q.clip,
                _ => None,
            })
            .expect("wrapped line carries a clip");
        assert!((clip.top - 124.0).abs() < 0.01 && (clip.bottom - 90.0).abs() < 0.01);
    }

    /// The lines draw at the frame's ARTWORK layer: a BACKGROUND texture, such as the chat
    /// window's hover box, behind them, an OVERLAY one in front.
    #[test]
    fn ring_lines_draw_above_the_frames_background_and_below_its_overlay() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        s.run(
            "local f = CreateFrame('ScrollingMessageFrame', 'CF')\n\
             f:SetPoint('BOTTOMLEFT', 32, 90)\n\
             f:SetWidth(430)\n\
             f:SetHeight(60)\n\
             local bg = f:CreateTexture('CFBg', 'BACKGROUND')\n\
             bg:SetTexture('Interface\\\\Bg')\n\
             bg:SetAllPoints(f)\n\
             local over = f:CreateTexture('CFOver', 'OVERLAY')\n\
             over:SetTexture('Interface\\\\Over')\n\
             over:SetAllPoints(f)\n\
             f:AddMessage('hello', 1, 1, 1)",
        )
        .unwrap();
        s.resolve();
        let quads = s.extract();
        let z_of = |path: &str| {
            quads
                .iter()
                .find_map(|q| match &q.content {
                    crate::script::QuadContent::Texture { path: Some(p), .. } if p == path => {
                        Some(q.z)
                    }
                    _ => None,
                })
                .expect("texture drew")
        };
        let line_z = quads
            .iter()
            .find_map(|q| match &q.content {
                crate::script::QuadContent::Text { text: Some(t), .. } if t == "hello" => Some(q.z),
                _ => None,
            })
            .expect("the ring line drew");
        let slot_z = quads
            .iter()
            .find(|q| matches!(q.content, crate::script::QuadContent::Frame))
            .map(|q| q.z)
            .expect("the frame slot drew");
        let (bg_z, over_z) = (z_of("Interface\\Bg"), z_of("Interface\\Over"));
        assert!(slot_z < bg_z, "the frame's own slot still leads");
        assert!(
            bg_z < line_z,
            "a BACKGROUND texture draws behind the messages"
        );
        assert!(line_z < over_z, "an OVERLAY texture draws in front of them");
    }

    #[test]
    fn width_change_invalidates_row_measures() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        s.run(
            "local f = CreateFrame('ScrollingMessageFrame', 'CF')\n\
             f:SetPoint('BOTTOMLEFT', 0, 0)\n\
             f:SetWidth(430)\n\
             f:SetHeight(120)",
        )
        .unwrap();
        s.run("CF:AddMessage('x', 1, 1, 1)").unwrap();
        s.resolve();
        let reqs = s.message_lines_needing_measure();
        assert_eq!(reqs.len(), 1);
        let answers: Vec<(u32, u32, u16, u64)> =
            reqs.iter().map(|r| (r.frame, r.index, 1, r.key)).collect();
        s.set_message_line_rows(&answers);
        assert!(s.message_lines_needing_measure().is_empty());
        s.run("CF:SetWidth(300)").unwrap();
        s.resolve();
        assert_eq!(
            s.message_lines_needing_measure().len(),
            1,
            "a width change re-requests the row count"
        );
    }

    #[test]
    fn faded_line_holds_its_rows_without_drawing() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        s.run(
            "local f = CreateFrame('ScrollingMessageFrame', 'CF')\n\
             f:SetPoint('BOTTOMLEFT', 0, 0)\n\
             f:SetWidth(430)\n\
             f:SetHeight(120)\n\
             f:SetTimeVisible(0)\n\
             f:SetFadeDuration(0)",
        )
        .unwrap();
        s.run("CF:AddMessage('doomed', 1, 1, 1)").unwrap();
        // Two ticks retire 'doomed' (phase 1, then the snap with no ramp); then fresh lines land.
        s.tick(0.1);
        s.tick(0.1);
        s.run("CF:SetTimeVisible(120)").unwrap();
        s.run("CF:AddMessage('a', 1, 1, 1)").unwrap();
        s.run("CF:AddMessage('b', 1, 1, 1)").unwrap();
        s.resolve();
        let quads = s.extract();
        let texts: Vec<(String, f32)> = quads
            .iter()
            .filter_map(|q| match (&q.content, q.rect) {
                (crate::script::QuadContent::Text { text: Some(t), .. }, Some(r)) => {
                    Some((t.clone(), r.bottom))
                }
                _ => None,
            })
            .collect();
        // 'doomed' draws nothing but keeps its band at the top; the live lines hold the bottom two.
        assert_eq!(texts.len(), 2);
        assert_eq!(texts[0].0, "b");
        assert!((texts[0].1 - 0.0).abs() < 0.01);
        assert_eq!(texts[1].0, "a");
        assert!((texts[1].1 - 14.0).abs() < 0.01);
    }

    #[test]
    fn hyperlink_span_release_fires_on_hyperlink_click() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        s.run(
            "local f = CreateFrame('ScrollingMessageFrame', 'CF')\n\
             f:SetPoint('BOTTOMLEFT', 0, 0)\n\
             f:SetWidth(430)\n\
             f:SetHeight(120)\n\
             f:SetScript('OnHyperlinkClick', function(self, link, text, button)\n\
                 CLICKED = link .. '#' .. text .. '#' .. button\n\
             end)",
        )
        .unwrap();
        s.run("CF:AddMessage('x', 1, 1, 1)").unwrap();
        s.resolve();
        let fh = s
            .extract()
            .iter()
            .find_map(|q| match (&q.content, q.target) {
                (
                    crate::script::QuadContent::Text { text: Some(t), .. },
                    crate::order::ZTarget::Frame(fh),
                ) if t == "x" => Some(fh),
                _ => None,
            })
            .expect("the ring line extracts");
        // The app feeds a span (a y-up rect) over [10..60]x[20..34]; a release inside fires the
        // handler with (link, markup, button), one outside does not.
        s.set_link_spans(vec![(
            fh,
            crate::layout::Rect::new(20.0, 10.0, 34.0, 60.0),
            "item:7073".to_string(),
            "|Hitem:7073|h[Broken Fang]|h".to_string(),
        )]);
        s.mouse_button(30.0, 25.0, "LeftButton", true);
        s.mouse_button(30.0, 25.0, "LeftButton", false);
        assert_eq!(
            s.eval::<String>("return CLICKED or ''").unwrap(),
            "item:7073#|Hitem:7073|h[Broken Fang]|h#LeftButton"
        );
        // Outside the span: no new fire.
        s.run("CLICKED = nil").unwrap();
        s.mouse_button(200.0, 100.0, "LeftButton", true);
        s.mouse_button(200.0, 100.0, "LeftButton", false);
        assert_eq!(
            s.eval::<String>("return CLICKED or 'none'").unwrap(),
            "none"
        );
    }

    #[test]
    fn add_chat_message_targets_the_named_scroll_frame() {
        let mut s = UiScript::new().unwrap();
        s.run("CreateFrame('ScrollingMessageFrame', 'ChatTest')")
            .unwrap();
        assert!(s.add_chat_message("ChatTest", "hello", 1.0, 1.0, 1.0));
        assert_eq!(
            s.eval::<i64>("return ChatTest:GetNumMessages()").unwrap(),
            1
        );
        // A missing frame or a non-message frame is a no-op.
        assert!(!s.add_chat_message("Nope", "x", 1.0, 1.0, 1.0));
        s.run("CreateFrame('Frame', 'PlainFrame')").unwrap();
        assert!(!s.add_chat_message("PlainFrame", "x", 1.0, 1.0, 1.0));
    }

    /// The id is stack index 6 with rgb and index 3 without (`0x792b13`, `0x792b48`), so
    /// `AceConsole-2.0`'s `AddMessage(text, nil, nil, nil, nil, 5)` stores 0.
    #[test]
    fn the_addmessage_id_index_follows_the_rgb_leg() {
        let s = UiScript::new().unwrap();
        s.run(
            "BenillaMF = CreateFrame('ScrollingMessageFrame') \
             BenillaMF:AddMessage('shorthand', 5) \
             BenillaMF:AddMessage('aceconsole', nil, nil, nil, nil, 5) \
             BenillaMF:AddMessage('coloured', 1, 1, 1, 7)",
        )
        .unwrap();
        let frame: mlua::Table = s.eval("return BenillaMF").unwrap();
        let ids: Vec<u32> = super::with_smf(s.lua(), &frame, |smf| {
            smf.lines.iter().map(|l| l.id).collect()
        })
        .unwrap();
        assert_eq!(
            ids,
            vec![5, 0, 7],
            "shorthand id from index 3; AceConsole's nils are id 0; the coloured line's id 7"
        );
    }

    #[test]
    fn update_color_by_id_recolours_only_that_ids_lines() {
        let mut smf = ScrollingMessageState::default();
        smf.add_with_id("a".into(), 1.0, 1.0, 1.0, 11);
        smf.add_with_id("b".into(), 1.0, 1.0, 1.0, 1);
        smf.add_with_id("c".into(), 1.0, 1.0, 1.0, 11);
        smf.add("d".into(), 1.0, 1.0, 1.0);
        let gen = smf.lines_gen;
        assert_eq!(smf.update_color_by_id(11, 0.0, 0.5, 1.0), 2);
        let colors: Vec<[u8; 3]> = smf.lines.iter().map(|l| l.color).collect();
        assert_eq!(
            colors,
            vec![
                [0, 128, 255],
                [255, 255, 255],
                [0, 128, 255],
                [255, 255, 255]
            ]
        );
        assert_ne!(smf.lines_gen, gen, "a recolour is a redraw");
        assert_eq!(smf.update_color_by_id(11, 0.0, 0.5, 1.0), 0, "idempotent");
        assert_eq!(smf.update_color_by_id(99, 0.0, 0.0, 0.0), 0, "no such id");
        // Id 0 matches nothing (`0x788250`). REPLY's chat type has id 0 and `UPDATE_CHAT_COLOR`
        // mirrors WHISPER into it, so without the guard every id-less line turns whisper pink.
        assert_eq!(
            smf.update_color_by_id(0, 1.0, 0.5, 1.0),
            0,
            "id 0 never reaches the record walk"
        );
        assert_eq!(
            smf.lines[3].color,
            [255, 255, 255],
            "the colourless line stays the frame's own colour"
        );

        // Through Lua: argument 5 tags the line, and a bad id is silent.
        let s = UiScript::new().unwrap();
        s.run(
            "local f = CreateFrame('ScrollingMessageFrame') \
             f:AddMessage('a', 1, 1, 1, 11) f:AddMessage('b') \
             f:UpdateColorByID(11, 0, 0.5, 1) f:UpdateColorByID('x', 0, 0, 0)",
        )
        .unwrap();
    }

    #[test]
    fn lua_addmessage_and_scroll_surface() {
        let s = UiScript::new().unwrap();
        s.run("CreateFrame('ScrollingMessageFrame', 'CF')").unwrap();
        s.run("CF:SetMaxLines(128)").unwrap();
        assert_eq!(s.eval::<i64>("return CF:GetMaxLines()").unwrap(), 128);
        s.run("CF:AddMessage('one', 1, 1, 1)").unwrap();
        s.run("CF:AddMessage('two')").unwrap(); // rgb-less form defaults white
        assert_eq!(s.eval::<i64>("return CF:GetNumMessages()").unwrap(), 2);
        assert!(s.eval::<bool>("return CF:AtBottom()").unwrap());
        s.run("CF:ScrollUp()").unwrap();
        assert!(!s.eval::<bool>("return CF:AtBottom()").unwrap());
        s.run("CF:ScrollToBottom()").unwrap();
        assert!(s.eval::<bool>("return CF:AtBottom()").unwrap());
        s.run("CF:Clear()").unwrap();
        assert_eq!(s.eval::<i64>("return CF:GetNumMessages()").unwrap(), 0);
    }

    /// A settled frame's sweep hashes no lines; a text change reopens it through the line
    /// generation, a width change through the environment hash.
    #[test]
    fn a_settled_message_frame_hashes_no_lines_until_something_moves() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        s.run(
            "local f = CreateFrame('ScrollingMessageFrame', 'CF')\n\
             f:SetPoint('BOTTOMLEFT', 32, 90)\n\
             f:SetWidth(430)\n\
             f:SetHeight(34)",
        )
        .unwrap();
        s.run("CF:AddMessage('one', 1, 1, 1)").unwrap();
        s.run("CF:AddMessage('two', 1, 1, 1)").unwrap();
        s.resolve();
        // Unanswered requests re-request: no token may be stored while keys mismatch.
        let first = s.message_lines_needing_measure();
        assert_eq!(first.len(), 2);
        let again = s.message_lines_needing_measure();
        assert_eq!(again.len(), 2, "unanswered lines must keep re-requesting");
        let answers: Vec<(u32, u32, u16, u64)> =
            again.iter().map(|r| (r.frame, r.index, 1, r.key)).collect();
        s.set_message_line_rows(&answers);
        assert!(s.message_lines_needing_measure().is_empty());
        // Settled: the next sweep skips the frame whole.
        let hashed_before = s.model_mut().msg_lines_hashed;
        assert!(s.message_lines_needing_measure().is_empty());
        assert_eq!(
            s.model_mut().msg_lines_hashed,
            hashed_before,
            "a settled frame's sweep must hash no lines"
        );
        // A text change reopens through the generation.
        s.run("CF:AddMessage('three', 1, 1, 1)").unwrap();
        let reqs = s.message_lines_needing_measure();
        assert_eq!(reqs.len(), 1, "the new line re-surfaces");
        assert_eq!(reqs[0].text, "three");
        let answers: Vec<(u32, u32, u16, u64)> =
            reqs.iter().map(|r| (r.frame, r.index, 1, r.key)).collect();
        s.set_message_line_rows(&answers);
        assert!(s.message_lines_needing_measure().is_empty());
        // A width change reopens through the environment: every line re-keys.
        s.run("CF:SetWidth(300)").unwrap();
        s.resolve();
        let reqs = s.message_lines_needing_measure();
        assert_eq!(reqs.len(), 3, "a new wrap width re-measures every line");
        assert!(reqs.iter().all(|r| (r.wrap_width - 300.0).abs() < 0.5));
    }
}
