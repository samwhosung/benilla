//! The chat bubble ABI as a bubble addon reads it: pfUI's `modules/bubbles.lua` walks
//! `WorldFrame:GetChildren()`, takes an unnamed child whose first region's texture is
//! `ChatBubble-Background`, blanks every Texture, reads the FontString's text and colour, parents
//! its own frame to the bubble with `SetAllPoints` and hooks `OnShow`.

use crate::script::{
    BubbleGeometry, BubbleState, MeasureRequest, QuadContent, TextMeasure, UiScript,
};

/// Each character `PER_CHAR` wide and each line `LINE_H` tall, wrapped greedily.
struct BlockFont;

const PER_CHAR: f32 = 6.0;
const LINE_H: f32 = 13.0;

impl TextMeasure for BlockFont {
    fn measure(&mut self, req: &MeasureRequest) -> (f32, f32, f32) {
        let natural = req.text.chars().count() as f32 * PER_CHAR;
        match req.wrap_width {
            Some(w) if w > 0.0 && natural > w => {
                let rows = (natural / w).ceil();
                (w, rows * LINE_H, natural)
            }
            _ => (natural, LINE_H, natural),
        }
    }
}

/// A VM with a `WorldFrame` to hang bubbles off and a font engine to measure their text.
fn vm() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.set_text_measurer(Box::new(BlockFont));
    // Stock `Fonts.xml` sets it; a plate or bubble built without it has no font and draws no text.
    s.run(r#"NAMEPLATE_FONT = "Fonts\\FRIZQT__.TTF""#).unwrap();
    s.run(r#"WorldFrame = CreateFrame("WorldFrame", "WorldFrame") WorldFrame:SetAllPoints()"#)
        .unwrap();
    s.resolve();
    s
}

const BORDER: f32 = 16.0;
const MARGIN: f32 = 13.0;

fn geometry() -> BubbleGeometry {
    BubbleGeometry {
        border: BORDER,
        margin: MARGIN,
        wrap_cap: 256.0,
        text_height: LINE_H,
        device_px: 1.0,
    }
}

fn say(key: u64, line: u64, text: &str) -> BubbleState {
    BubbleState {
        key,
        line,
        text: text.to_string(),
        colour: [1.0, 1.0, 1.0],
        shown: true,
        alpha: 255,
        seat: (500.0, 300.0),
    }
}

/// Drive the bubbles and settle the layout, the way one app frame does.
fn drive(s: &mut UiScript, states: &[BubbleState]) {
    s.sync_chat_bubbles(geometry(), states);
    s.resolve();
}

/// A say puts an anonymous `Frame` under the `WorldFrame`, in its stratum, shown.
#[test]
fn a_say_puts_an_anonymous_frame_in_the_worldframe_child_list() {
    let mut s = vm();
    assert_eq!(
        s.eval::<i64>("return WorldFrame:GetNumChildren()").unwrap(),
        0
    );
    drive(&mut s, &[say(7, 1, "Well met!")]);
    assert_eq!(
        s.eval::<i64>("return WorldFrame:GetNumChildren()").unwrap(),
        1
    );
    assert!(s
        .eval::<bool>(
            r#"local b = WorldFrame:GetChildren()
               return b:GetObjectType() == "Frame" and b:GetName() == nil
                  and b:IsShown() == 1 and b:GetParent() == WorldFrame
                  and b:GetFrameStrata() == WorldFrame:GetFrameStrata()
                  and b:GetFrameLevel() == 2"#
        )
        .unwrap());
}

/// Eleven regions in creation order: the backdrop's background and eight border pieces
/// (`0x4b0a35` → `0x77e8d0`), the tail (`0x4b0a57`), the text (`0x4b0b14`).
#[test]
fn the_eleven_regions_run_backdrop_then_tail_then_text() {
    let mut s = vm();
    drive(&mut s, &[say(7, 1, "Well met!")]);
    let rows: Vec<String> = s
        .eval::<Vec<String>>(
            r#"local b = WorldFrame:GetChildren()
               local out, regions = {}, { b:GetRegions() }
               for i = 1, table.getn(regions) do
                 local r = regions[i]
                 local tex = r:GetObjectType() == "Texture" and (r:GetTexture() or "nil") or "-"
                 table.insert(out, r:GetObjectType() .. " " .. r:GetDrawLayer() .. " " .. tex)
               end
               return out"#,
        )
        .unwrap();
    let mut want =
        vec!["Texture BACKGROUND Interface\\Tooltips\\ChatBubble-Background".to_string()];
    want.extend(std::iter::repeat_n(
        "Texture BORDER Interface\\Tooltips\\ChatBubble-Backdrop".to_string(),
        8,
    ));
    want.push("Texture ARTWORK Interface\\Tooltips\\ChatBubble-Tail".to_string());
    want.push("FontString ARTWORK -".to_string());
    assert_eq!(rows, want);
}

/// `0x4b1600`: the line's text in its chat-type colour, wrapped at the cap and floored at two
/// border units. The frame hugs the text by the margin (`0x4b0b6b`, `0x4b0b94`), the text's
/// BOTTOM sits on the seat (`0x4b157a`), and the tail hangs TOPRIGHT from the frame's BOTTOM
/// lifted a quarter border (`0x4b0af1`).
#[test]
fn the_text_is_the_line_in_its_colour_and_the_frame_hugs_it() {
    let mut s = vm();
    let mut yell = say(7, 1, "Well met!");
    yell.colour = [1.0, 0.25, 0.25];
    drive(&mut s, &[yell]);
    let (text, r, g, b): (String, f32, f32, f32) = s
        .eval(
            r#"local regions = { (WorldFrame:GetChildren()):GetRegions() }
               local fs = regions[11]
               local r, g, b = fs:GetTextColor()
               return fs:GetText(), r, g, b"#,
        )
        .unwrap();
    assert_eq!(text, "Well met!");
    let read = crate::color::quant::read32;
    crate::assert_near!((r, g, b), (read(1.0), read(0.25), read(0.25)));

    let rect = |s: &UiScript, what: &str| -> (f32, f32, f32, f32) {
        s.eval(&format!(
            "local b = WorldFrame:GetChildren() local regions = {{ b:GetRegions() }}
             local o = {what}
             return o:GetLeft(), o:GetBottom(), o:GetWidth(), o:GetHeight()"
        ))
        .unwrap()
    };
    // Nine characters, 54 wide: over the two-border floor and under the cap.
    let fs = rect(&s, "regions[11]");
    crate::assert_near!(
        fs,
        (473.0, 300.0, 54.0, LINE_H),
        "the text's BOTTOM on the seat"
    );
    let frame = rect(&s, "b");
    crate::assert_near!(
        frame,
        (460.0, 287.0, 54.0 + 2.0 * MARGIN, LINE_H + 2.0 * MARGIN),
        "the margin on all four sides"
    );
    let tail = rect(&s, "regions[10]");
    crate::assert_near!(
        tail,
        (
            500.0 - BORDER,
            287.0 + BORDER / 4.0 - BORDER,
            BORDER,
            BORDER
        ),
        "TOPRIGHT on the frame's BOTTOM, a quarter border up"
    );

    // Short text takes the floor, long text the cap and a second line.
    drive(&mut s, &[say(7, 2, "hi")]);
    crate::assert_near!(rect(&s, "regions[11]").2, 2.0 * BORDER);
    let long = "x".repeat(50);
    drive(&mut s, &[say(7, 3, &long)]);
    let fs = rect(&s, "regions[11]");
    crate::assert_near!((fs.2, fs.3), (256.0, 2.0 * LINE_H));

    // The frame's left edge lands on the device grid, wherever the seat falls.
    let mut off = say(7, 4, "Well met!");
    off.seat = (500.3, 300.3);
    s.sync_chat_bubbles(
        BubbleGeometry {
            device_px: 2.0,
            ..geometry()
        },
        &[off],
    );
    s.resolve();
    let frame = rect(&s, "b");
    crate::assert_near!(
        ((frame.0 * 2.0).fract(), (frame.1 * 2.0).fract()),
        (0.0, 0.0)
    );
}

/// pfUI's skin: blank every Texture, keep the text, cover the bubble with its own frame. Nothing
/// of the stock art draws afterwards, and the blanking survives the bubble's reuse.
#[test]
fn a_pfui_skin_leaves_none_of_the_stock_art_drawn() {
    let mut s = vm();
    drive(&mut s, &[say(7, 1, "Well met!")]);
    let stock = |s: &UiScript| -> Vec<String> {
        s.extract()
            .iter()
            .filter_map(|q| match &q.content {
                QuadContent::Texture { path: Some(p), .. } if p.contains("ChatBubble") => {
                    Some(p.clone())
                }
                _ => None,
            })
            .collect()
    };
    assert_eq!(
        stock(&s).len(),
        10,
        "the stock bubble draws its ten textures"
    );

    s.run(
        r#"
        function PfSkin()
          local kids = { WorldFrame:GetChildren() }
          for i = 1, table.getn(kids) do
            local f = kids[i]
            local first = not f:GetName() and f:GetRegions()
            if first and first.GetTexture
               and first:GetTexture() == "Interface\\Tooltips\\ChatBubble-Background"
               and not f.pfSkin then
              local regions = { f:GetRegions() }
              for j = 1, table.getn(regions) do
                local r = regions[j]
                if r:GetObjectType() == "Texture" then r:SetTexture(nil)
                elseif r:GetObjectType() == "FontString" then f.pfText = r end
              end
              f.pfSkin = CreateFrame("Frame", nil, f)
              f.pfSkin:SetAllPoints(f)
              PfRead = f.pfText:GetText()
              PfR, PfG, PfB = f.pfText:GetTextColor()
              f:SetScript("OnShow", function() PfShows = (PfShows or 0) + 1 end)
            end
          end
        end
        PfSkin()
    "#,
    )
    .unwrap();
    s.resolve();
    assert_eq!(s.eval::<String>("return PfRead").unwrap(), "Well met!");
    assert!(stock(&s).is_empty(), "every stock texture blanked");
    assert!(
        s.extract().iter().any(|q| matches!(
            &q.content,
            QuadContent::Text { text: Some(t), .. } if t == "Well met!"
        )),
        "the text still draws"
    );
    assert!(s
        .eval::<bool>(
            r#"local b = WorldFrame:GetChildren()
               return b.pfSkin:GetLeft() == b:GetLeft() and b.pfSkin:GetTop() == b:GetTop()
                  and b.pfSkin:GetWidth() == b:GetWidth() and b.pfSkin:GetHeight() == b:GetHeight()"#
        )
        .unwrap());

    // The line ends and a new one reuses the skinned bubble: OnShow fires, the art stays blank.
    drive(&mut s, &[]);
    drive(&mut s, &[say(9, 2, "Hail.")]);
    assert_eq!(s.eval::<i64>("return PfShows").unwrap(), 1);
    assert!(
        stock(&s).is_empty(),
        "a reused bubble keeps the skin's blanking"
    );
}

/// The pool (`0x4b1870`): a finished bubble is hidden (`0x4b1590`), keeps its place in the child
/// list and its parent, and the next line takes the oldest retired one back, firing `OnShow`
/// (`0x4b0ea0`'s `Show`).
#[test]
fn a_bubble_is_reused_never_reparented_and_hidden_when_done() {
    let mut s = vm();
    drive(&mut s, &[say(1, 1, "one"), say(2, 1, "two")]);
    s.run(
        r#"Shows, Hides = {}, {}
           local a, b = WorldFrame:GetChildren()
           a.mark, b.mark = "A", "B"
           for _, f in ipairs({ a, b }) do
             local f = f
             f:SetScript("OnShow", function() table.insert(Shows, f.mark) end)
             f:SetScript("OnHide", function() table.insert(Hides, f.mark) end)
           end"#,
    )
    .unwrap();

    // The first line ends, then the second: each frame hides, keeps its parent and its slot.
    drive(&mut s, &[say(2, 1, "two")]);
    drive(&mut s, &[]);
    assert!(s
        .eval::<bool>(
            r#"local a, b = WorldFrame:GetChildren()
               return WorldFrame:GetNumChildren() == 2 and a.mark == "A" and b.mark == "B"
                  and not a:IsShown() and not b:IsShown()
                  and a:GetParent() == WorldFrame and b:GetParent() == WorldFrame
                  and Hides[1] == "A" and Hides[2] == "B""#
        )
        .unwrap());

    // A new line takes the bubble retired first, and its OnShow fires.
    drive(&mut s, &[say(3, 1, "three")]);
    assert!(s
        .eval::<bool>(
            r#"local a, b = WorldFrame:GetChildren()
               local regions = { a:GetRegions() }
               return a:IsShown() == 1 and not b:IsShown() and Shows[1] == "A"
                  and regions[11]:GetText() == "three""#
        )
        .unwrap());
    assert_eq!(
        s.chat_bubble_pool_len(),
        2,
        "no frame was made for the reuse"
    );

    // A new line from the same speaker replaces the bubble: its frame goes to the free list's
    // tail and the line takes the head, here the frame retired before it (`0x608bb4`, `0x608bbf`).
    drive(&mut s, &[say(3, 2, "again")]);
    assert!(s
        .eval::<bool>(
            r#"local a, b = WorldFrame:GetChildren()
               return Hides[3] == "A" and Shows[2] == "B" and not a:IsShown()"#
        )
        .unwrap());

    // Out of range the fade ends hidden; back in range it shows again (`0x4b0d9f`, `0x4b0ea0`).
    let mut away = say(3, 2, "again");
    away.shown = false;
    drive(&mut s, &[away]);
    assert!(s
        .eval::<bool>(
            r#"local _, b = WorldFrame:GetChildren() return not b:IsShown() and Hides[4] == "B""#
        )
        .unwrap());
    drive(&mut s, &[say(3, 2, "again")]);
    assert!(s
        .eval::<bool>(
            r#"local _, b = WorldFrame:GetChildren() return b:IsShown() == 1 and Shows[3] == "B""#
        )
        .unwrap());
    assert_eq!(s.chat_bubble_pool_len(), 2);
}

/// The per-frame pass (`0x4b12d5`-`0x4b1312`): levels 2, 3, … down the list, farthest first,
/// without carrying children (`0x76a4f0(level, 0)`); the fade is the frame's own alpha.
#[test]
fn bubbles_take_levels_from_two_in_list_order_and_their_fade_alpha() {
    let mut s = vm();
    let mut near = say(1, 1, "near");
    near.alpha = 127;
    drive(&mut s, &[say(2, 1, "far"), near.clone()]);
    let levels = |s: &UiScript| -> (i64, i64) {
        s.eval(
            r#"local far, near = WorldFrame:GetChildren()
               return far:GetFrameLevel(), near:GetFrameLevel()"#,
        )
        .unwrap()
    };
    assert_eq!(levels(&s), (2, 3));
    assert_eq!(
        s.eval::<f64>("local _, near = WorldFrame:GetChildren() return near:GetAlpha()")
            .unwrap(),
        127.0 / 255.0
    );
    drive(&mut s, &[near, say(2, 1, "far")]);
    assert_eq!(
        levels(&s),
        (3, 2),
        "the order is the app's, re-stamped every frame"
    );
}

/// The text slides with the seat on the device grid, so it skips the UI grid snap, as a plate's.
#[test]
fn the_bubble_text_carries_the_world_seat() {
    let mut s = vm();
    drive(&mut s, &[say(7, 1, "Well met!")]);
    assert!(s.extract().iter().any(|q| matches!(
        &q.content,
        QuadContent::Text { text: Some(t), world_seat: true, .. } if t == "Well met!"
    )));
}

/// A window resize re-sizes a live bubble in place: the backdrop's unit, the tail, the margin and
/// the width floor follow, and its eleven regions stay the same objects.
#[test]
fn a_resize_re_sizes_the_live_bubble_in_place() {
    let mut s = vm();
    drive(&mut s, &[say(7, 1, "hi")]);
    s.run(r#"local r = { (WorldFrame:GetChildren()):GetRegions() } First, Tail = r[1], r[10]"#)
        .unwrap();
    let wider = BubbleGeometry {
        border: 20.0,
        margin: 15.0,
        ..geometry()
    };
    s.sync_chat_bubbles(wider, &[say(7, 1, "hi")]);
    s.resolve();
    let (same, frame_w, tail_w, edge_w): (bool, f32, f32, f32) = s
        .eval(
            r#"local b = WorldFrame:GetChildren() local r = { b:GetRegions() }
               return r[1] == First and r[10] == Tail, b:GetWidth(), r[10]:GetWidth(),
                      r[6]:GetWidth()"#,
        )
        .unwrap();
    assert!(same, "the regions are kept, not rebuilt");
    assert_eq!(frame_w, 2.0 * 20.0 + 2.0 * 15.0, "the new floor and margin");
    assert_eq!(tail_w, 20.0);
    assert_eq!(edge_w, 20.0, "the corner piece takes the new edge size");
}

/// Lines that end together go back to the pool in the active list's order, farthest first, as
/// the reference's walk destroys them (`0x4b1138`); the next line takes the first of them.
#[test]
fn ended_lines_retire_farthest_first() {
    let mut s = vm();
    // Pooled A then B, but B is the farther, so first in the list.
    drive(&mut s, &[say(1, 1, "a")]);
    drive(&mut s, &[say(2, 1, "b"), say(1, 1, "a")]);
    s.run(r#"local a, b = WorldFrame:GetChildren() a.mark, b.mark = "A", "B""#)
        .unwrap();
    drive(&mut s, &[]);
    drive(&mut s, &[say(3, 1, "c")]);
    assert_eq!(
        s.eval::<String>(
            r#"local a, b = WorldFrame:GetChildren()
               if a:IsShown() then return a.mark end return b.mark"#
        )
        .unwrap(),
        "B",
        "the farther bubble retired first, so it is reused first"
    );
}

/// The text's font is the `NAMEPLATE_FONT` global's, read when the bubble is built (`0x4b0b36` →
/// `0x703bf0`); no global, no font.
#[test]
fn the_text_takes_the_nameplate_font_global() {
    let mut s = vm();
    s.run(r#"NAMEPLATE_FONT = "Fonts\\ARIALN.TTF""#).unwrap();
    drive(&mut s, &[say(1, 1, "a")]);
    assert_eq!(
        s.eval::<String>(
            "local r = { (WorldFrame:GetChildren()):GetRegions() } return (r[11]:GetFont())"
        )
        .unwrap(),
        "Fonts\\ARIALN.TTF"
    );

    let mut bare = vm();
    bare.run("NAMEPLATE_FONT = nil").unwrap();
    drive(&mut bare, &[say(1, 1, "a")]);
    assert!(bare
        .eval::<bool>(
            "local r = { (WorldFrame:GetChildren()):GetRegions() } return r[11]:GetFont() == nil"
        )
        .unwrap());
}

/// The lookup honours an `_G` `__index` (`lua_gettable`, `0x704373`), which may call any verb, the
/// Model's included; it is made for a new bubble only, and outside the Model's borrow.
#[test]
fn a_global_index_metamethod_may_touch_the_model_during_a_bubbles_font_lookup() {
    let mut s = vm();
    s.run(
        r#"NAMEPLATE_FONT = nil
           hits = 0
           setmetatable(getfenv(0), { __index = function(t, k)
               if k == "NAMEPLATE_FONT" then
                   hits = hits + 1
                   CreateFrame("Frame", nil, UIParent):SetWidth(UIParent:GetWidth())
               end
               return nil
           end })"#,
    )
    .unwrap();
    drive(&mut s, &[say(1, 1, "a")]);
    assert_eq!(s.eval::<i64>("return hits").unwrap(), 1);
    // The same speaker's next line takes the freed frame; a second speaker needs a new one.
    drive(&mut s, &[say(1, 2, "b")]);
    assert_eq!(s.eval::<i64>("return hits").unwrap(), 1);
    drive(&mut s, &[say(1, 2, "b"), say(2, 1, "c")]);
    assert_eq!(s.eval::<i64>("return hits").unwrap(), 2);
}

/// The bubble text's justification as a Lua reader sees it.
fn text_justify(s: &mut UiScript) -> String {
    s.eval::<String>(
        r#"local regions = { (WorldFrame:GetChildren()):GetRegions() }
           return regions[11]:GetJustifyH()"#,
    )
    .unwrap()
}

/// `0x4b16e7`: only koKR left-justifies the text; every other locale keeps the FontString's
/// CENTER. A reused bubble takes the new line's justify alike, and the VM's seated locale is the
/// one read.
#[test]
fn only_a_korean_client_left_justifies_the_bubble_text() {
    for index in 0..8 {
        let mut s = vm();
        s.set_locale(index);
        drive(
            &mut s,
            &[say(7, 1, "Well met, traveller, and well met again!")],
        );
        let want = if index == 1 { "LEFT" } else { "CENTER" };
        assert_eq!(text_justify(&mut s), want, "locale {index}");
        drive(&mut s, &[say(7, 2, "Farewell.")]);
        assert_eq!(text_justify(&mut s), want, "locale {index}, second line");
    }
}
