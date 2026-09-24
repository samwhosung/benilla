//! The GameTooltip Lua verb table — [`install`] registers the widget methods (SetOwner /
//! AddLine / AddDoubleLine / SetText / ClearLines / SetMinimumWidth / SetPadding / FadeOut /
//! IsOwned / NumLines / the anchor law) over the parent module's line-pool mechanics, then
//! chains the item/unit content builders' method sets.

use mlua::{Lua, Table, Value, Variadic};

use crate::layout::{Anchor, Point};
use crate::script::binding_abi::optional_string;
use crate::script::object::{frame_handle_of, frame_wrapper};
use crate::script::region::region_handle_of;
use crate::script::Model;
use crate::widget::{KindState, TooltipAnchor, TOOLTIP_LINE_GAP, TOOLTIP_PAD};

use super::{
    append_line, bool_arg, clear_content, fire_cleared, full_alpha, hide_tooltip, now,
    parse_line_color, parse_line_tail, set_shown, text_of, tip_mut, with_tip, write_cell,
    REG_TOOLTIP_METHODS,
};

pub(in crate::script) fn install(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;

    // SetOwner(owner, anchorType [, x, y]) — the binding `0x5310d0` and the core it calls,
    // `0x52ffe0(owner_region, mode, dx, dy)`: SetAlpha(255) (`0x52fff4`), fade-disarm
    // (`0x530002`), `+0x314 = owner` (`0x53000c`), the mode (`0x530031`), the offsets, then the
    // anchor-apply `0x52fe90`. We clear the content too, because the engine fires
    // OnTooltipCleared: a fresh owner never inherits the last hover's lines or money.
    //
    // **The omitted / unreadable / unrecognised anchorType is mode 0 = ANCHOR_LEFT, silently.**
    // `0x53120d` writes 0 into the binding's local mode *before any compare runs*; `0x531214`'s
    // `lua_isstring` gate jumps the whole `SStrCmpI` chain when arg 3 is absent, nil, boolean or a
    // table; the chain's last `jne` reaches the same join untouched; and `[0x531221, 0x53133a)`
    // contains no `luaL_error` (earned zero, positive-controlled against the five this function
    // *does* have in its owner-validation prologue). `0x5313c0` then hands that local to the core.
    //
    // The core's own `mov [+0x318],7` at `0x530012` is **not** that default: it is an
    // unconditional pre-store that `0x530031` overwrites with the argument, and it survives only
    // when the owner pointer is NULL — the un-own path `0x530a60` takes on every hide.
    //
    // **The anchor-apply clears, and only ANCHOR_PRESERVE escapes it** (`0x52fe90` read
    // contiguously): a NULL owner returns at `0x52fe9e`; **mode 8 returns at `0x52fead`, before
    // the clear**; then `0x52feb8` tests the caller's arg1, which `0x530037` always passes as
    // **1** (`0x53002d push 0x1`), so the mode-7 skip at `0x52feba` is dead on this path and
    // `0x52fec2 call 0x767ed0` (ClearAllPoints) runs for **every mode 0..7**; and only then does
    // `0x52fed0 ja` send 6 and 7 home while 0..5 take the SetPoint jump table `0x52ffbc`.
    //
    // So the three arms below are: 0..5 clear-and-point, **6 and 7 clear**, 8 nothing. Decision
    // 2176 replaces 2142's open thread 1, which had it backwards on both counts — it read the
    // core's pre-store as the Lua default and read "no SetPoint" as "no clear"; the bytes refute
    // both.
    m.set(
        "SetOwner",
        lua.create_function(
            |lua, (this, owner, anchor, x, y): (Table, Table, Value, Option<f32>, Option<f32>)| {
                let h = frame_handle_of(lua, &this)?;
                let owner_h = frame_handle_of(lua, &owner)?;
                // The gate is `lua_isstring 0x6f3510`, whose whole body is `lua_type` then
                // `cmp eax,4 / cmp eax,3` — LUA_TSTRING or LUA_TNUMBER and nothing else — so the
                // anchor argument is read the reference's way, by COERCION, and a boolean, table,
                // function or userdata is indistinguishable from absent. Taking it as
                // `Option<String>` made mlua's converter the gate instead, and mlua raises on a
                // table: `Questie`'s `Tooltip:SetOwner(this, this)` (QuestieNotes.lua, the map-note
                // hover) died with "bad argument #3: error converting Lua table to String" on a
                // call the reference completes silently at mode 0. This function's doc comment has
                // stated the law since 2176 — only the signature disagreed.
                let anchor = optional_string(lua, &anchor);
                {
                    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                    clear_content(&mut model, h);
                    full_alpha(&mut model, h);
                    tip_mut(&mut model, h)?.owner = Some(owner_h);
                    let owner_id = model.frame_id(owner_h);
                    // The anchor mode is resolved ONCE, into the reference's own enum, and both
                    // the placement below and `GetAnchorType`'s answer are taken from that one
                    // value — so the plate and the getter can never tell different stories.
                    //
                    // Absent, unreadable and unrecognised all land on **ANCHOR_LEFT**, which is
                    // the binding's zero-initialised local (`0x53120d`) surviving the
                    // `lua_isstring` gate or the compare chain, with no raise. The warning on the
                    // unrecognised leg is ours and stays: the reference is silent, but nothing
                    // observable to Lua changes, and this exact warning is what made
                    // `ANCHOR_CURSOR` visible as a real ninth mode nine corpus files ask for
                    // rather than a typo (2142's open thread 2).
                    let anchor = match anchor
                        .as_deref()
                        .map(str::to_ascii_uppercase)
                        .as_deref()
                        .unwrap_or("")
                    {
                        "ANCHOR_LEFT" => TooltipAnchor::Left,
                        "ANCHOR_RIGHT" => TooltipAnchor::Right,
                        "ANCHOR_BOTTOMLEFT" => TooltipAnchor::BottomLeft,
                        "ANCHOR_BOTTOMRIGHT" => TooltipAnchor::BottomRight,
                        "ANCHOR_TOPLEFT" => TooltipAnchor::TopLeft,
                        "ANCHOR_TOPRIGHT" => TooltipAnchor::TopRight,
                        "ANCHOR_CURSOR" => TooltipAnchor::Cursor,
                        "ANCHOR_NONE" => TooltipAnchor::None,
                        "ANCHOR_PRESERVE" => TooltipAnchor::Preserve,
                        "" => TooltipAnchor::Left,
                        other => {
                            model.record_warning(format!(
                                "SetOwner: unknown anchor '{other}' (mode 0, ANCHOR_LEFT)"
                            ));
                            TooltipAnchor::Left
                        }
                    };
                    tip_mut(&mut model, h)?.anchor = anchor;
                    let pts = match anchor {
                        TooltipAnchor::Right => Some((Point::BottomLeft, Point::TopRight)),
                        TooltipAnchor::Left => Some((Point::BottomRight, Point::TopLeft)),
                        TooltipAnchor::TopRight => Some((Point::BottomRight, Point::TopRight)),
                        TooltipAnchor::TopLeft => Some((Point::BottomLeft, Point::TopLeft)),
                        TooltipAnchor::BottomRight => Some((Point::TopLeft, Point::BottomRight)),
                        TooltipAnchor::BottomLeft => Some((Point::TopRight, Point::BottomLeft)),
                        TooltipAnchor::Cursor | TooltipAnchor::None | TooltipAnchor::Preserve => {
                            None
                        }
                    };
                    // **Modes 6 and 7 CLEAR and stop; mode 8 does neither.** `0x52fe90` reaches
                    // `0x52fec2 call 0x767ed0` for every mode 0..7 — the mode-7 skip beside it is
                    // gated on an arg1 that `SetOwner`'s core always passes as 1 — and mode 8
                    // returned two compares earlier, at `0x52fead`, without touching anything.
                    //
                    // For ANCHOR_NONE that clear is what makes `GameTooltip_SetDefaultAnchor`
                    // (`SetOwner(owner, "ANCHOR_NONE")` → `ClearAllPoints()` → `SetPoint(...)`,
                    // the path of every action-bar hover) safe: a stale owner anchor cannot win
                    // the frame the caller forgets to re-point. For ANCHOR_CURSOR it is the
                    // opening move of a placement the per-frame update finishes
                    // (`script::tooltip::cursor_anchor`).
                    //
                    // Dropping every anchor is a retarget to the EMPTY target set, and it names
                    // its node like any other. It matters here more than
                    // anywhere: left on the conservative touch, this line was a whole-graph
                    // derivation on every action button the cursor crossed and on nothing else —
                    // exactly the shape the director's recorder reported.
                    if matches!(anchor, TooltipAnchor::Cursor | TooltipAnchor::None) {
                        let dropped = match model.layout_inputs.get_mut(&h) {
                            Some(input) if !input.anchors.is_empty() => {
                                Some(input.anchors.drain(..).map(|a| a.relative_to).collect())
                            }
                            _ => None,
                        };
                        if let Some(old_targets) = dropped {
                            let old_targets: Vec<u32> = old_targets;
                            model.touch_layout_retarget_frame(h, &old_targets, &[]);
                        }
                    }
                    if let Some((own, rel)) = pts {
                        let new =
                            Anchor::new(own, owner_id, rel, x.unwrap_or(0.0), y.unwrap_or(0.0));
                        let input = model.layout_inputs.entry(h).or_default();
                        // The no-op compare keeps the per-frame SetOwner idiom (the bag
                        // hover's OnUpdate re-enter) from dirtying tier 1 by itself; the
                        // content clear above already reports its own writes.
                        let same = input.anchors.len() == 1
                            && crate::script::object::anchor_bits_eq(&input.anchors[0], &new);
                        if !same {
                            // The plate re-points at the button under the cursor, which moves
                            // an EDGE — structural under 1388, and therefore a whole-graph
                            // derivation on every bag slot and every spellbook button a hover
                            // sweep crossed. It names its node now: the old
                            // and new target lists are both right here, so the cached graph's
                            // edges are re-pointed instead of thrown away.
                            let old_targets: Vec<u32> =
                                input.anchors.iter().map(|a| a.relative_to).collect();
                            input.anchors = vec![new];
                            model.touch_layout_retarget_frame(h, &old_targets, &[owner_id]);
                        }
                    }
                }
                fire_cleared(lua, h);
                Ok(())
            },
        )?,
    )?;
    // GameTooltip:BenillaGetTooltipOwner() — **ours, and the prefix is what makes it honest.**
    // 1.12 registers `IsOwned` (ask about ONE frame) and no getter at all, so there is no
    // reference spelling for "which frame owns this plate" to be faithful to; a `GetOwner` here
    // would be an unexplained superset an addon can feature-detect (1188, and the census that
    // moved it, 2142). Under the `Benilla` prefix it is unreachable by accident from an addon
    // that means to call a WoW function. Its one caller is the dev-only hover recorder
    // (`benilla-app/src/hover_log.rs`), which needs the owner's NAME per frame so a trace reads
    // the same names the director sees.
    m.set(
        "BenillaGetTooltipOwner",
        lua.create_function(|lua, this: Table| {
            let owner = with_tip(lua, &this, |t| t.owner)?;
            let id = {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                owner
                    .filter(|&h| model.arena.frame(h).is_some())
                    .map(|h| model.frame_id(h))
            };
            match id {
                Some(id) => Ok(Value::Table(frame_wrapper(lua, id)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;
    // GetAnchorType() — ONE string, the mode the last SetOwner placed this plate by
    // (`0x5313e0`, table `0x854198`, argc 1, arity 1, kinds `(string)`; it reads `[+0x318]` back
    // through the 9-entry name table `0x531530`). The kinds column has no nil alternative and this
    // does not answer one: a tooltip nothing has owned yet answers `"ANCHOR_NONE"`, which is
    // [`TooltipAnchor`]'s default and the same mode the reference's own SetOwner core falls back to.
    //
    // Two corpus callers, both unguarded, both raising against this VM until now, and they are the
    // two shapes the verb has: `pfUI/modules/tooltip.lua:97` compares against ONE mode
    // (`if GameTooltip:GetAnchorType() == "ANCHOR_NONE" then` — its OnShow reposition, so an equality
    // on the exact reference spelling), and `_Nameplates/_Nameplates.lua:479` compares against a
    // mode it computed (`if GameTooltip:GetAnchorType() ~= Anchor then GameTooltip:SetOwner(Column,
    // Anchor) end` — a re-SetOwner suppressor, so an answer that never equals what SetOwner was
    // given would re-own the plate on every OnUpdate). Both need the string to round-trip through
    // the setter verbatim, which is why the mode is resolved once up there and read straight back
    // here rather than re-derived from the plate's anchors.
    m.set(
        "GetAnchorType",
        lua.create_function(|lua, this: Table| {
            let anchor = with_tip(lua, &this, |t| t.anchor)?;
            Ok(anchor.name())
        })?,
    )?;
    // IsOwned(frame) — the hover re-enter loop's gate (ref ContainerFrame.lua OnUpdate): true
    // only while `frame` is the live owner; the owner drops on hide, so a hidden tooltip never
    // resurrects from a stale OnUpdate.
    m.set(
        "IsOwned",
        lua.create_function(|lua, (this, frame): (Table, Table)| {
            let owner = with_tip(lua, &this, |t| t.owner)?;
            let fh = frame_handle_of(lua, &frame)?;
            Ok(owner == Some(fh))
        })?,
    )?;

    // SetText(text, r, g, b, alpha, textWrap) — the byte-pinned `0x531b90` signature (alpha is
    // a REAL parsed argument here, its own isnumber gate, default 1.0 — unlike AddLine's forced
    // opaque; GameTooltip_AddNewbieTip passes `(text, r, g, b, 1, 1)`). `text` is REQUIRED — a
    // non-string raises the binding's own 'Usage: SetText("text" [, color])' FrameScript error
    // and adds no line. No colour args → the engine default GOLD. Clears, writes line 1,
    // SHOWS — the corpus never calls Show after SetText (PaperDoll's empty-slot text relies
    // on it).
    m.set(
        "SetText",
        lua.create_function(|lua, (this, args): (Table, Variadic<Value>)| {
            let h = frame_handle_of(lua, &this)?;
            if !matches!(
                args.first(),
                Some(Value::String(_)) | Some(Value::Number(_)) | Some(Value::Integer(_))
            ) {
                return Err(mlua::Error::RuntimeError(
                    "Usage: SetText(\"text\" [, color])".into(),
                ));
            }
            let text = text_of(args.first());
            let [r, g, b, _] = parse_line_color(args.get(1), args.get(2), args.get(3));
            let a = match args.get(4) {
                Some(Value::Number(n)) => *n as f32,
                Some(Value::Integer(i)) => *i as f32,
                _ => 1.0,
            };
            let wrap = bool_arg(args.get(5));
            {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                clear_content(&mut model, h);
            }
            fire_cleared(lua, h);
            append_line(lua, &this, (text, [r, g, b, a]), None, wrap)?;
            set_shown(lua, h, true);
            Ok(())
        })?,
    )?;
    // AppendText(text) — `0x531e30`. The reference's bag/bar tooltips build a title with SetText
    // and then hang the keybinding on the end of that same line rather than adding a second one:
    //
    //     GameTooltip:SetText(TEXT(BACKPACK_TOOLTIP), 1.0, 1.0, 1.0);
    //     if ( GetBindingKey("TOGGLEBACKPACK") ) then
    //         GameTooltip:AppendText(" "..NORMAL_FONT_COLOR_CODE.."("..key..")"..FONT_COLOR_CODE_CLOSE)
    //     end
    //
    // — ContainerFrame.xml l.188-206 (the bag window's portrait button, one arm per bag id) and
    // MainMenuBarBagButtons.xml l.96 / .lua l.91 (the bar's). Both files run off the player's own
    // chain (1751), so those are live call sites in this VM, not transcriptions we could adjust.
    //
    // LINE 1's left cell, appended in place, keeping its colour: the escape codes in the argument
    // are what colour the suffix, which only works if it lands INSIDE an existing line. A tooltip
    // with no lines yet is a no-op rather than a raise — the reference's own callers always SetText
    // first, and the empty case is unconfirmed in the binary.
    //
    // INFERRED, unconfirmed in the binary: that `0x531e30` targets line 1 rather than the last line
    // added. Every attested call site sets exactly one line before appending, so the two readings
    // are indistinguishable from the corpus alone; line 1 is chosen because the reference's own
    // name for the target is `GameTooltipTextLeft1`. Re-measure is unconditional either way.
    m.set(
        "AppendText",
        lua.create_function(|lua, (this, text): (Table, Value)| {
            let h = frame_handle_of(lua, &this)?;
            let extra = text_of(Some(&text));
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let lh = match model.arena.frame(h).map(|f| &f.kind_state) {
                Some(KindState::Tooltip(t)) if t.num_lines > 0 => t.left_lines[0],
                Some(KindState::Tooltip(_)) => return Ok(()),
                _ => return Err(mlua::Error::runtime("not a GameTooltip")),
            };
            let (current, color) = match model.region_data.get(&lh) {
                Some(d) => (
                    d.text.clone().unwrap_or_default(),
                    d.vertex_color.unwrap_or([1.0, 1.0, 1.0, 1.0]),
                ),
                None => (String::new(), [1.0, 1.0, 1.0, 1.0]),
            };
            write_cell(&mut model, lh, &(current + &extra), color);
            Ok(())
        })?,
    )?;
    // AddLine(text [, r, g, b [, wrap]]) — positional; the corpus' archaic `(text, "", r, g, b)`
    // shape has a non-number in the r-slot, so the real binding drops the colour tail and the
    // line renders the default GOLD (the ref's zone/taxi/repair tooltips — director-matched).
    // Appends without showing.
    m.set(
        "AddLine",
        lua.create_function(|lua, (this, args): (Table, Variadic<Value>)| {
            let text = text_of(args.first());
            let (color, wrap) = parse_line_tail(&args[1.min(args.len())..]);
            append_line(lua, &this, (text, color), None, wrap)
        })?,
    )?;
    // AddDoubleLine(textL, textR, rL, gL, bL, rR, gR, bR [, wrap]) — the two-column line
    // (slot|type, damage|speed), byte-pinned `0x531840`: each side's colour gates on ITS OWN
    // r-slot isnumber (default gold, g/b ungated → 0.0, alpha forced opaque), and the wrap arg
    // is dead weight — the core forces wrap=0 whenever the right text is non-empty. The right
    // column right-flushes in the layout pass.
    m.set(
        "AddDoubleLine",
        lua.create_function(
            |lua, (this, tl, tr, cl): (Table, Value, Value, Variadic<Value>)| {
                let left_color = parse_line_color(cl.first(), cl.get(1), cl.get(2));
                let right_color = parse_line_color(cl.get(3), cl.get(4), cl.get(5));
                append_line(
                    lua,
                    &this,
                    (text_of(Some(&tl)), left_color),
                    Some((text_of(Some(&tr)), right_color)),
                    false,
                )
            },
        )?,
    )?;
    m.set(
        "ClearLines",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                clear_content(&mut model, h);
            }
            fire_cleared(lua, h);
            Ok(())
        })?,
    )?;
    m.set(
        "NumLines",
        lua.create_function(|lua, this: Table| with_tip(lua, &this, |t| t.num_lines as i64))?,
    )?;
    // AddFontStrings(left, right) — adopt two caller-made FontStrings as the next line pair
    // (`0x530c40`; the parent module's doc already names it as how the real class grows past its
    // template's 30 declared pairs).
    //
    // The scan-tooltip idiom, and the reason this landed with the font-object work rather than
    // separately: `Gratuity-2.0.lua:56-59` builds its whole 30-line hidden tooltip out of
    // `tt:CreateFontString()` + `:SetFontObject(GameFontNormal)` + `tt:AddFontStrings(l, r)`, so
    // publishing the font globals only moved that library's death one line down. Five corpus addons
    // sit behind those four lines.
    //
    // The pair is placed exactly as an engine-grown one (hidden, left/right justified, hung off the
    // previous line) — the layout pass owns line geometry either way — but the regions' FONTS are
    // left alone, since choosing them is the entire point of the caller doing this by hand.
    m.set(
        "AddFontStrings",
        lua.create_function(|lua, (this, left, right): (Table, Table, Table)| {
            let h = frame_handle_of(lua, &this)?;
            let lh = region_handle_of(lua, &left)?;
            let rh = region_handle_of(lua, &right)?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let frame_id = model.frame_id(h);
            let prev_left = match model.arena.frame(h).map(|f| &f.kind_state) {
                Some(KindState::Tooltip(t)) => t.left_lines.last().copied(),
                _ => return Err(mlua::Error::runtime("not a GameTooltip")),
            };
            let anchor = match prev_left {
                None => Anchor::new(
                    Point::TopLeft,
                    frame_id,
                    Point::TopLeft,
                    TOOLTIP_PAD,
                    -TOOLTIP_PAD,
                ),
                Some(prev) => {
                    let prev_id = model.region_id(prev);
                    Anchor::new(
                        Point::TopLeft,
                        prev_id,
                        Point::BottomLeft,
                        0.0,
                        -TOOLTIP_LINE_GAP,
                    )
                }
            };
            let left_id = model.region_id(lh);
            {
                let d = model.region_data.entry(lh).or_default();
                d.hidden = true;
                d.justify.set_h(crate::script::JustifyH::Left);
                d.anchors = vec![anchor];
            }
            {
                let d = model.region_data.entry(rh).or_default();
                d.hidden = true;
                d.justify.set_h(crate::script::JustifyH::Right);
                d.anchors = vec![Anchor::new(Point::Right, left_id, Point::Right, 0.0, 0.0)];
            }
            model.touch_layout(); // two line rows entered the layout graph (decision 0740)
            let t = tip_mut(&mut model, h)?;
            t.left_lines.push(lh);
            t.right_lines.push(rh);
            Ok(())
        })?,
    )?;
    // SetMinimumWidth(w) — a floor on the auto-sized width (the ref's SetTooltipMoney calls it
    // with the money row's width so coins never overhang the plate).
    m.set(
        "SetMinimumWidth",
        lua.create_function(|lua, (this, w): (Table, f32)| {
            with_tip(lua, &this, |t| t.min_width = w.max(0.0))
        })?,
    )?;
    // SetPadding(w) — extra auto-size width (ref ItemRefTooltip OnLoad: SetPadding(16) keeps the
    // corner close button off the text). A frame property — persists across content clears.
    m.set(
        "SetPadding",
        lua.create_function(|lua, (this, w): (Table, f32)| {
            with_tip(lua, &this, |t| t.padding = w.max(0.0))
        })?,
    )?;
    // FadeOut() — the world-mouseover loss ramp (alpha 1→0 over TOOLTIP_FADE_SECS, INTERIM
    // constant, then a real hide). Fresh content or Show cancels it.
    m.set(
        "FadeOut",
        lua.create_function(|lua, this: Table| {
            let t = now(lua);
            with_tip(lua, &this, |tip| {
                if tip.fade_start.is_none() {
                    tip.fade_start = Some(t);
                }
            })
        })?,
    )?;
    // Show/Hide — shadow the shared pair (kind tables resolve first) to keep the tooltip's
    // state honest: Show cancels a fade at full alpha; Hide drops the owner + content and fires
    // OnTooltipCleared (the ref clears its money row through exactly that script).
    // Show is an EXISTENCE GATE, not a plain show. `0x530a80` — the CGameTooltip override of
    // `vtbl+0x88`, which is the slot a Lua `:Show()` lands in (the generic widget Show path
    // `0x7a3350` sets `[+0xd0]=1` then calls slot `+0x88`) — shows only when BOTH `+0x314` (the
    // owner) and `+0x31c` (the line count) are non-zero; otherwise it calls its own `vtbl+0x84`
    // effective-hide `0x530a60`, which is the SetOwner core with a NULL owner, so the plate is
    // hidden AND un-owned and OnTooltipCleared fires. Evaluated at Show time only — it is never a
    // visibility poll (`0x530a80`).
    //
    // Without the gate an addon that reaches `Show()` having added no lines leaves a VISIBLE
    // empty plate — and, because `layout_tooltips` skips a zero-line plate rather than collapsing
    // it, one still wearing the last hover's width and height. That is what `Questie`'s tracker
    // does: `QuestieTracker.lua`'s quest-button OnEnter has a dead zone between its two arms (an
    // in-progress quest whose objective text IS in its database adds nothing) and then calls
    // `Tooltip:Show()` unconditionally. On the reference `0x530a80` swallows it; we drew the
    // empty plate.
    //
    // `show_or_hide_empty` is the same law for the app-answered content asks, and deliberately
    // does NOT un-own (its re-enter repaint needs the owner). This path is the reference's own,
    // so it takes the reference's full effective-hide.
    m.set(
        "Show",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let live = {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                full_alpha(&mut model, h);
                tip_mut(&mut model, h)
                    .map(|t| t.owner.is_some() && t.num_lines > 0)
                    .unwrap_or(false)
            };
            match live {
                true => set_shown(lua, h, true),
                false => hide_tooltip(lua, h),
            }
            Ok(())
        })?,
    )?;
    m.set(
        "Hide",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            hide_tooltip(lua, h);
            Ok(())
        })?,
    )?;

    // The item content channels (BenillaSetItemById/SetBagItem/SetMerchantItem/SetBuybackItem) live in
    // their own module — the one shared renderer + the red usable law (decision 0274 P1).
    crate::script::tooltip_item::install_methods(lua, &m)?;
    // The spell/aura/action content channels (SetSpell/SetShapeshift/SetPlayerBuff/SetAction) —
    // the verified spell-builder law (decision 0274 P2, grounded by 0276).
    crate::script::tooltip_spell::install_methods(lua, &m)?;
    // The talent channel (SetTalent) — the spell builder with the talent interleave.
    crate::script::talent::install_tooltip_method(lua, &m)?;
    // The unit content channel (SetUnit + the world-mouseover drivers) — the verified unit law
    // (decision 0274 P3, grounded by 0276).
    crate::script::tooltip_unit::install_methods(lua, &m)?;

    lua.set_named_registry_value(REG_TOOLTIP_METHODS, m)?;
    Ok(())
}
