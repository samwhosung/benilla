//! The `ScrollingMessageFrame` methods over [`ScrollingMessageState`]: `CSimpleMessageScrollFrame`
//! (ctor `0x787670`), the chat window's class. A ring of `maxLines` that drops the oldest, colours
//! quantized `trunc(x*255+0.5)` with alpha forced opaque, a fade that ticks only at the bottom, and
//! every scroll call re-arming the displayed lines' fade (`0x788b80` when refused at an end,
//! `0x788af0` when it moves), which is what brings a faded chat back.

use mlua::{Lua, MultiValue, Table, Value};

use super::{channel, is_number};
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

/// Run `f` over the state with the viewport's row count, which every scroll needs: each re-arms
/// the fade of the lines on display ([`ScrollingMessageState::reset_all_fade_times`]), and `AtTop`
/// reads the layout's flag.
fn with_viewport<T>(
    lua: &Lua,
    this: &Table,
    f: impl FnOnce(&mut ScrollingMessageState, usize) -> T,
) -> mlua::Result<T> {
    let h = frame_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let viewport_rows = UiScript::message_viewport_rows(&model, h);
    let frame = model
        .arena
        .frame_mut(h)
        .ok_or_else(|| mlua::Error::runtime("stale frame handle"))?;
    match &mut frame.kind_state {
        KindState::ScrollingMessage(smf) => Ok(f(smf, viewport_rows)),
        _ => Err(mlua::Error::runtime("not a ScrollingMessageFrame")),
    }
}

/// Run a scroll call: [`with_viewport`] for an op with no answer.
fn scroll(
    lua: &Lua,
    this: &Table,
    op: impl FnOnce(&mut ScrollingMessageState, usize),
) -> mlua::Result<()> {
    with_viewport(lua, this, op)
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
                let has_rgb = [&r, &g, &b].iter().all(|v| is_number(lua, v).is_some());
                // The chat-type index (`ChatFrame.lua` passes `info.id`), stack index 6 or 3, a
                // `lua_isnumber`-gated `_ftol` (`0x792b1b`, `0x792b4f`); 0 when the gate fails.
                let id = is_number(lua, if has_rgb { &id } else { &r })
                    .map_or(0, |n| crate::script::binding_abi::ftol(n) as u32);
                let (r, g, b) = if has_rgb {
                    (channel(lua, &r), channel(lua, &g), channel(lua, &b))
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
                // All four arguments are `lua_isnumber`-gated (`0x793345`-`0x793388`); any failure
                // is a silent no-op, and the id is an `_ftol` (`0x7933a1`).
                if ![&id, &r, &g, &b]
                    .iter()
                    .all(|v| is_number(lua, v).is_some())
                {
                    return Ok(());
                }
                let id =
                    crate::script::binding_abi::ftol(is_number(lua, &id).unwrap_or(0.0)) as u32;
                let (r, g, b) = (channel(lua, &r), channel(lua, &g), channel(lua, &b));
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
    // AtTop (`0x793130`) pushes 1 (`0x7931c3`) or nil (`0x7931d6`); AtBottom (`0x7931f0`) pushes 1
    // (`0x793283`) or nil (`0x793296`).
    m.set(
        "AtTop",
        lua.create_function(|lua, this: Table| {
            with_viewport(lua, &this, |smf, rows| {
                crate::script::binding_abi::flag(smf.at_top(rows))
            })
        })?,
    )?;
    m.set(
        "AtBottom",
        lua.create_function(|lua, this: Table| {
            with_smf(lua, &this, |smf| {
                crate::script::binding_abi::flag(smf.at_bottom())
            })
        })?,
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
        lua.create_function(|lua, (this, args): (Table, MultiValue)| {
            // `GetBoolOrDefault` (`0x6f1c10`, `0x7939a0`) with default 1: a bare call enables.
            let on = crate::script::binding_abi::bool_or_default(args.front(), true);
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
        crate::script::font_block::FontKind::Instance,
        None,
    )?;

    lua.set_named_registry_value(REG_SCROLLINGMESSAGEFRAME_METHODS, m)?;

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

    /// The link flag's producers (`+0x3b8`): a new line's relayout (`0x788234`), a line fading out
    /// (`0x788538`), once, and any scroll entry, refused or not (`0x788bf1`, `0x788a8e`). The
    /// fade's ramp alone raises nothing.
    #[test]
    fn the_link_flag_rises_on_relayout_fade_out_and_scroll() {
        let mut s = ScrollingMessageState {
            time_visible: 0.5,
            fade_duration: 1.0,
            ..Default::default()
        };
        assert!(!s.links_dirty, "the ctor clears it");
        s.add("x".into(), 1.0, 1.0, 1.0);
        assert!(std::mem::take(&mut s.links_dirty), "AddMessage relayouts");
        s.tick(0.6);
        s.tick(0.5);
        assert!(
            s.lines[0].alpha > 0.0 && !s.links_dirty,
            "mid-ramp: no flag"
        );
        s.tick(0.6);
        assert_eq!(s.lines[0].alpha, 0.0);
        assert!(std::mem::take(&mut s.links_dirty), "the hide raises it");
        s.tick(0.6);
        assert!(!s.links_dirty, "a hidden line is not hidden again");
        s.scroll_down(4);
        assert!(
            s.links_dirty,
            "a scroll refused at the bottom re-arms and raises it"
        );
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
        // A frame showing every line is at the top already (`+0x350`, `0x788a37`).
        assert!(s.at_top(8));
        s.scroll_up(8);
        assert_eq!(s.scroll_offset, 0, "refused: the oldest is on screen");
        // One row high, the oldest shows only at offset 2; scrolling past it is refused.
        for _ in 0..10 {
            s.scroll_up(1);
        }
        assert!(s.at_top(1));
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
        s.scroll_up(1); // viewing one line back (bottom row = l2)
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
        // 4 rows: the newest (1) and the middle (3) fill it. A message that does not fit whole
        // ends the walk (`0x7889c0`-`0x7889c5`) unless it is the anchored one.
        assert_eq!(s.displayed_count(4), 2);
        assert_eq!(
            s.displayed_count(3),
            1,
            "a partly fitting middle line is out"
        );
        assert_eq!(s.displayed_count(1), 1);
        assert_eq!(s.displayed_count(0), 1, "the anchored message always shows");
        // `PageUp` (`0x7885b0`) scrolls up once per displayed line and then down once, so a page
        // is the displayed count less one, and one displayed line nets nothing.
        s.page_up(4); // 2 displayed: up moves, the second up is refused (the oldest shows)
        assert_eq!(s.scroll_offset, 1);
        s.page_up(1); // 1 displayed: up, down
        assert_eq!(s.scroll_offset, 1, "a page of one line nets 0");
        // `PageDown` (`0x7885e0`): a step refused at the bottom ends the page with no step back.
        s.page_down(4); // 2 displayed: down moves, down is refused
        assert!(s.at_bottom());
        s.page_down(4); // 2 displayed: both refused at the first
        assert!(s.at_bottom());
    }

    /// A refused step ends the page where it stands (`0x7885ca`), with no step back; an empty
    /// frame moves nothing and re-arms nothing (`0x788619`).
    #[test]
    fn a_page_stopped_by_an_end_takes_no_step_back() {
        let mut s = ScrollingMessageState {
            max_lines: 8,
            ..Default::default()
        };
        assert!(!s.step_up(4), "an empty frame answers 0");
        s.page_up(4);
        s.page_down(4);
        assert!(!s.links_dirty, "an empty frame is left alone");
        for n in 0..4 {
            s.add(format!("l{n}"), 1.0, 1.0, 1.0);
        }
        // Two rows high: the oldest shows from offset 2, where every step up is refused.
        s.scroll_up(2);
        s.scroll_up(2);
        assert_eq!(s.scroll_offset, 2);
        assert!(s.at_top(2));
        // The first up is refused, so the page ends with no down after it.
        s.page_up(2);
        assert_eq!(s.scroll_offset, 2);
        // One row high there, `PageDown` is a down and an up: it nets 0.
        s.page_down(1);
        assert_eq!(s.scroll_offset, 2, "down then up nets 0");
    }

    /// With two or more lines displayed and room to move, the page runs its whole loop and then
    /// steps back: a page is the displayed count less one (`0x7885b0`, `0x7885e0`).
    #[test]
    fn a_page_with_room_runs_the_loop_and_steps_back() {
        let mut s = ScrollingMessageState {
            max_lines: 8,
            ..Default::default()
        };
        for n in 0..8 {
            s.add(format!("l{n}"), 1.0, 1.0, 1.0);
        }
        s.page_up(3); // 3 displayed: up, up, up, down
        assert_eq!(s.scroll_offset, 2);
        s.page_up(3);
        assert_eq!(s.scroll_offset, 4);
        s.page_down(3); // down, down, down, up
        assert_eq!(s.scroll_offset, 2);
    }

    /// `AtTop` answers nil on an empty frame: the constructor, `Clear` and `SetMaxLines` zero the
    /// flag (`0x787775`, `0x7882ce`, `0x787eb1`).
    #[test]
    fn at_top_is_nil_on_an_empty_frame() {
        let s = UiScript::new().unwrap();
        s.run(
            "CF = CreateFrame('ScrollingMessageFrame', 'CF')\n\
             EMPTY = CF:AtTop()\n\
             CF:AddMessage('a') CF:Clear()\n\
             CLEARED = CF:AtTop()\n\
             CF:AddMessage('a') CF:SetMaxLines(4)\n\
             WIPED = CF:AtTop()",
        )
        .unwrap();
        for name in ["EMPTY", "CLEARED", "WIPED"] {
            assert!(
                s.eval::<bool>(&format!("return {name} == nil")).unwrap(),
                "{name}"
            );
        }
    }

    /// `ScrollToTop` on a single message anchors the newest (`0x788799`-`0x7887aa`); a frame
    /// already at the bottom skips the re-arm (`0x788aff`), which two messages do not.
    #[test]
    fn scroll_to_top_with_one_message_skips_the_re_arm_at_the_bottom() {
        let faded = |count: usize| {
            let mut s = ScrollingMessageState {
                time_visible: 0.0,
                fade_duration: 4.0,
                ..Default::default()
            };
            for n in 0..count {
                s.add(format!("l{n}"), 1.0, 1.0, 1.0);
            }
            s.tick(2.0);
            s.links_dirty = false;
            s.scroll_to_top(8);
            s
        };
        let one = faded(1);
        assert_eq!(one.lines[0].alpha, 127.0 / 255.0, "left as it was");
        assert!(one.links_dirty, "the links still rebuild");
        let two = faded(2);
        assert_eq!(two.scroll_offset, 1);
        assert_eq!(two.lines[0].alpha, 1.0, "the oldest was re-armed");
    }

    /// `ScrollUp` is refused once the oldest message is on screen (`0x78861c`-`0x788624`, the flag
    /// the relayout sets at `0x788a3c`), not when it is the bottom row; `ScrollToTop` (`0x788690`)
    /// anchors the oldest. `AtTop` (`0x793130`) reads the same flag.
    #[test]
    fn scroll_up_stops_when_the_oldest_message_shows() {
        let mut s = ScrollingMessageState {
            max_lines: 8,
            ..Default::default()
        };
        for n in 0..5 {
            s.add(format!("l{n}"), 1.0, 1.0, 1.0);
        }
        // Three rows show 3 of 5: l4 l3 l2, then l3 l2 l1, then l2 l1 l0.
        assert!(!s.at_top(3));
        s.scroll_up(3);
        s.scroll_up(3);
        assert_eq!(s.scroll_offset, 2);
        assert!(s.at_top(3), "l0 is on screen");
        s.scroll_up(3);
        assert_eq!(s.scroll_offset, 2, "refused");
        s.scroll_to_top(3);
        assert_eq!(s.scroll_offset, 4, "the oldest as the anchor");
    }

    /// The Lua `AtTop` follows the viewport the frame's rect gives.
    #[test]
    fn at_top_reads_the_layout_through_lua() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        s.run(
            "CF = CreateFrame('ScrollingMessageFrame', 'CF')\n\
             CF:SetPoint('BOTTOMLEFT', 32, 90)\n\
             CF:SetWidth(430)\n\
             CF:SetHeight(44)\n\
             for n = 1, 5 do CF:AddMessage('m' .. n) end",
        )
        .unwrap();
        s.resolve();
        // 44 px hold three rows of 14.
        assert!(s.eval::<bool>("return CF:AtTop() == nil").unwrap());
        s.run("CF:ScrollUp() CF:ScrollUp()").unwrap();
        assert!(s.eval::<bool>("return CF:AtTop() == 1").unwrap());
        // Shrunk to two rows, the oldest message no longer shows.
        s.run("CF:SetHeight(30)").unwrap();
        s.resolve();
        assert!(
            s.eval::<bool>("return CF:AtTop() == nil").unwrap(),
            "a shorter frame places fewer messages, so the oldest drops off"
        );
    }

    /// A scrolling frame `CF` of the given height holding `messages`, bottom left at (32, 90),
    /// after the host answered each message's row count with `rows(index)`: what it draws as
    /// `(text, bottom, top)` in draw order, and the count its layout reports.
    fn laid_out(
        height: u32,
        messages: &[&str],
        rows: impl Fn(u32) -> u16,
    ) -> (Vec<(String, f32, f32)>, usize) {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        s.run(&format!(
            "local f = CreateFrame('ScrollingMessageFrame', 'CF')\n\
             f:SetPoint('BOTTOMLEFT', 32, 90)\n\
             f:SetWidth(430)\n\
             f:SetHeight({height})"
        ))
        .unwrap();
        for m in messages {
            s.run(&format!("CF:AddMessage('{m}', 1, 1, 1)")).unwrap();
        }
        s.resolve();
        let answers: Vec<(u32, u32, u16, u64)> = s
            .message_lines_needing_measure()
            .iter()
            .map(|r| (r.frame, r.index, rows(r.index), r.key))
            .collect();
        s.set_message_line_rows(&answers);
        let mut fh = None;
        let texts = s
            .extract()
            .iter()
            .filter_map(|q| match (&q.content, q.rect, q.target) {
                (
                    crate::script::QuadContent::Text { text: Some(t), .. },
                    Some(r),
                    crate::order::ZTarget::Frame(h),
                ) => {
                    fh = Some(h);
                    Some((t.clone(), r.bottom, r.top))
                }
                _ => None,
            })
            .collect();
        let model = s.model_mut();
        let fh = fh.expect("a line drew");
        let viewport = UiScript::message_viewport_rows(&model, fh);
        let count = match &model.arena.frame(fh).expect("the frame").kind_state {
            crate::widget::KindState::ScrollingMessage(smf) => smf.displayed_count(viewport),
            _ => unreachable!("a scrolling frame"),
        };
        (texts, count)
    }

    /// The layout places the anchored message, then only what fits whole (`0x7889b3`-`0x7889c5`).
    /// Default font, pitch 14: at 34 px (two rows and a bit) 'wrapped' starts inside the frame at 104
    /// and runs to 132, past its top at 124, so it is neither drawn nor counted, and 'old' beyond it
    /// is not either.
    #[test]
    fn emit_stacks_wrapped_rows_and_hides_a_partial_top() {
        let rows = |i| if i == 1 { 2 } else { 1 };
        let (texts, count) = laid_out(34, &["old", "wrapped", "new"], rows);
        assert_eq!(texts.len(), 1, "only the anchored message: {texts:?}");
        assert_eq!(texts[0].0, "new");
        assert!((texts[0].1 - 90.0).abs() < 0.01 && (texts[0].2 - 104.0).abs() < 0.01);
        assert_eq!(count, 1, "the displayed count agrees with the draw");

        // 48 px (three rows and a bit) holds 'new' and 'wrapped' whole; 'old' would be the
        // fraction at the top edge, so it stays out. Farthest displayed line first, the link
        // rebuild's walk (`0x788c8d`-`0x788d7d`).
        let (texts, count) = laid_out(48, &["old", "wrapped", "new"], rows);
        assert_eq!(count, 2);
        assert_eq!(texts.len(), 2, "{texts:?}");
        assert_eq!(texts[0].0, "wrapped");
        assert!((texts[0].1 - 104.0).abs() < 0.01 && (texts[0].2 - 132.0).abs() < 0.01);
        assert_eq!(texts[1].0, "new");
        assert!((texts[1].1 - 90.0).abs() < 0.01 && (texts[1].2 - 104.0).abs() < 0.01);
    }

    /// The anchored message shows even when it alone is taller than the frame (`0x7889c0`): the
    /// frame's 20 px hold a row and a bit, the newest message wraps to three rows, and it draws
    /// from the bottom edge while the older one does not.
    #[test]
    fn the_anchored_message_shows_though_taller_than_the_frame() {
        let (texts, count) = laid_out(20, &["old", "tall"], |i| if i == 1 { 3 } else { 1 });
        assert_eq!(texts.len(), 1, "{texts:?}");
        assert_eq!(texts[0].0, "tall");
        assert!((texts[0].1 - 90.0).abs() < 0.01 && (texts[0].2 - 132.0).abs() < 0.01);
        assert_eq!(count, 1);
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
        assert_eq!(texts[0].0, "a");
        assert!((texts[0].1 - 14.0).abs() < 0.01);
        assert_eq!(texts[1].0, "b");
        assert!((texts[1].1 - 0.0).abs() < 0.01);
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
             f:SetScript('OnHyperlinkClick', function()\n\
                 local link, text, button = arg1, arg2, arg3\n\
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
