//! The always-up world-state readout (stock `Interface\FrameXML\WorldStateFrame.xml`) —
//! report B190's second half, driven the way the client drives it: rows pushed, then
//! `UPDATE_WORLD_STATES` fired.
//!
//! What these pin is the *frame's* half of the contract — that it reads the ten returns in the
//! right order, swaps the dynamic icon on the row's own state, and empties itself when a scope
//! admits nothing. Which rows exist at all is [`crate::world_state_ui`]'s, tested there against
//! the real `WorldStateUI.dbc`.

use benilla_ui::script::{QuadContent, ScriptValue, UiScript, WorldStateUiView};

use super::test_ui::load_ui as load_xml;

pub(crate) fn harness() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in [
        "Interface\\FrameXML\\Fonts.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        r"Interface\FrameXML\UIParent.xml",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\BasicControls.xml",
        "Interface\\FrameXML\\LocaleProperties.lua",
        "Interface\\FrameXML\\StaticPopup.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        "Interface\\FrameXML\\WorldStateFrame.xml", // the reference's own (1972)
    ] {
        load_xml(&s, f);
    }
    // The outdoor readout is gated on the options uvar `HIDE_OUTDOOR_WORLD_STATE == "0"`
    // (UIOptionsFrame.lua's default, ours in OptionsFrame.xml's uvar block); a harness without
    // the options file says so itself.
    s.run("HIDE_OUTDOOR_WORLD_STATE = \"0\"").unwrap();
    s
}

/// An Eastern Plaguelands-shaped row.
pub(crate) fn row(icon: &str, text: &str, tooltip: &str) -> WorldStateUiView {
    WorldStateUiView {
        ui_state: 1,
        text: text.into(),
        icon: icon.into(),
        tooltip: tooltip.into(),
        ..Default::default()
    }
}

/// Every shown text string, in draw order.
fn texts(s: &mut UiScript) -> Vec<String> {
    s.resolve();
    s.extract()
        .iter()
        .filter_map(|q| match &q.content {
            QuadContent::Text { text: Some(t), .. } if !t.is_empty() => Some(t.clone()),
            _ => None,
        })
        .collect()
}

/// Every shown texture path, in draw order.
fn textures(s: &mut UiScript) -> Vec<String> {
    s.resolve();
    s.extract()
        .iter()
        .filter_map(|q| match &q.content {
            QuadContent::Texture { path: Some(p), .. } => Some(p.clone()),
            _ => None,
        })
        .collect()
}

pub(crate) fn push(s: &mut UiScript, rows: Vec<WorldStateUiView>) {
    s.set_world_state_ui(rows);
    s.tick(0.0);
    s.fire_event("UPDATE_WORLD_STATES", vec![]);
}

/// The Eastern Plaguelands readout: two labelled tower counters, each with its faction icon.
#[test]
fn the_tower_counters_draw_with_their_icons() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.fire_event("PLAYER_ENTERING_WORLD", vec![ScriptValue::Str("".into())]);
    assert!(
        !texts(&mut s).iter().any(|t| t.contains("Towers")),
        "nothing before a push"
    );

    push(
        &mut s,
        vec![
            row(
                "Interface\\WorldStateFrame\\AllianceTower",
                "Towers Controlled: 3",
                "Alliance Towers Controlled",
            ),
            row(
                "Interface\\WorldStateFrame\\HordeTower",
                "Towers Controlled: 1",
                "Horde Towers Controlled",
            ),
        ],
    );

    let shown = texts(&mut s);
    assert!(
        shown.contains(&"Towers Controlled: 3".to_string())
            && shown.contains(&"Towers Controlled: 1".to_string()),
        "both counters on screen: {shown:?}"
    );
    let art = textures(&mut s);
    assert!(
        art.iter().any(|p| p.contains("AllianceTower"))
            && art.iter().any(|p| p.contains("HordeTower")),
        "both faction icons on screen: {art:?}"
    );
}

/// A scope that admits nothing empties the readout — and the pooled rows from the busier scope
/// are hidden rather than left painting a stale count.
#[test]
fn leaving_the_zone_clears_the_readout() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.fire_event("PLAYER_ENTERING_WORLD", vec![ScriptValue::Str("".into())]);
    push(
        &mut s,
        vec![row(
            "Interface\\WorldStateFrame\\AllianceTower",
            "Towers Controlled: 3",
            "Alliance Towers Controlled",
        )],
    );
    assert!(texts(&mut s).contains(&"Towers Controlled: 3".to_string()));

    push(&mut s, Vec::new());
    assert!(
        !texts(&mut s).iter().any(|t| t.contains("Towers")),
        "no stale count left painting: {:?}",
        texts(&mut s)
    );
    assert!(
        !textures(&mut s).iter().any(|p| p.contains("AllianceTower")),
        "and no orphaned icon"
    );
}

/// The dynamic icon is a SECOND slot, not a swap (`GetWorldStateUIInfo 0x4c5a70`; decision
/// 1604). The `Icon` column and the `DynamicIcon` column feed two different regions — a 42x42
/// static slot and a 32x32 button off the row's right edge — and only `uiState == 2`, the
/// flag-taken value, lights the second one. The first pass replaced the static art whenever the
/// state was non-zero, which fired on the ordinary state 1 and hid the faction shield that should
/// never have moved; both halves of that are pinned here.
#[test]
fn the_dynamic_icon_is_a_second_slot_lit_only_by_the_taken_state() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.fire_event("PLAYER_ENTERING_WORLD", vec![ScriptValue::Str("".into())]);

    let flag_row = |ui_state| WorldStateUiView {
        ui_state,
        text: "2/3".into(),
        icon: "Interface\\TargetingFrame\\UI-PVP-Alliance".into(),
        dynamic_icon: "Interface\\WorldStateFrame\\HordeFlag".into(),
        tooltip: "Alliance flag captures".into(),
        dynamic_tooltip: "Horde flag has been picked up".into(),
        ..Default::default()
    };

    // A zero state is not a quiet row but NO row: the stock `WorldStateAlwaysUpFrame_Update`
    // draws only `state > 0` (WorldStateFrame.lua) — our transcription had painted the shield at
    // zero too, and the reference's file retired that (1972).
    push(&mut s, vec![flag_row(0)]);
    let art = textures(&mut s);
    assert!(
        !art.iter().any(|p| p.contains("UI-PVP-Alliance")),
        "state 0 — the row is not drawn at all: {art:?}"
    );
    for quiet in [1, 3] {
        push(&mut s, vec![flag_row(quiet)]);
        let art = textures(&mut s);
        assert!(
            art.iter().any(|p| p.contains("UI-PVP-Alliance")),
            "state {quiet} — the faction shield never moves: {art:?}"
        );
        assert!(
            !art.iter().any(|p| p.contains("HordeFlag")),
            "state {quiet} is not the flag-taken state, so nothing lights: {art:?}"
        );
    }

    push(&mut s, vec![flag_row(2)]);
    let art = textures(&mut s);
    assert!(
        art.iter().any(|p| p.contains("UI-PVP-Alliance")),
        "taken — the shield is STILL there, beside the flag: {art:?}"
    );
    assert!(
        art.iter()
            .any(|p| p == "Interface\\WorldStateFrame\\HordeFlag"),
        "taken — the enemy flag lights up: {art:?}"
    );
    assert!(
        art.iter()
            .any(|p| p == "Interface\\WorldStateFrame\\HordeFlagFlash"),
        "…with its ADD-blend flash overlay, whose path is the icon's plus `Flash`: {art:?}"
    );

    // The pulse runs: the flash overlay's alpha ramps up across the first half-second and back
    // down across the second, and the handler that does it raises nothing.
    let flash_alpha = |s: &mut UiScript| {
        s.resolve();
        s.extract()
            .iter()
            .find(|q| {
                matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.ends_with("HordeFlagFlash"))
            })
            .map(|q| q.alpha)
            .expect("the flash overlay is on screen")
    };
    let at_zero = flash_alpha(&mut s);
    s.tick(0.25);
    let quarter = flash_alpha(&mut s);
    s.tick(0.25);
    let half = flash_alpha(&mut s);
    s.tick(0.5);
    let full = flash_alpha(&mut s);
    assert!(
        at_zero < quarter && quarter < half,
        "the flash ramps in over its first half-second: {at_zero}, {quarter}, {half}"
    );
    assert!(
        full < half,
        "…and back out over the second: {half} then {full}"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // And it goes out again when the flag is returned.
    push(&mut s, vec![flag_row(1)]);
    assert!(!textures(&mut s).iter().any(|p| p.contains("HordeFlag")));
}

/// A row with no icon at all (the Eastern Plaguelands progress line) still draws its text — the
/// icon is optional, and the row must not collapse when the DBC column is empty.
#[test]
fn a_row_without_an_icon_still_shows_its_text() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.fire_event("PLAYER_ENTERING_WORLD", vec![ScriptValue::Str("".into())]);
    push(
        &mut s,
        vec![WorldStateUiView {
            ui_state: 1,
            text: "Progress: 60".into(),
            ..Default::default()
        }],
    );
    assert!(texts(&mut s).contains(&"Progress: 60".to_string()));
}

/// An `ExtendedUI` row is not a text row at all — the stock update routes it to the extended
/// kit (`ExtendedUI["CAPTUREPOINT"]`), which builds a capture BAR and never prints the text.
/// Our transcription had shown the text; the reference's file draws the bar (1972).
#[test]
fn a_capture_point_row_is_a_bar_not_a_line_of_text() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.fire_event("PLAYER_ENTERING_WORLD", vec![ScriptValue::Str("".into())]);
    push(
        &mut s,
        vec![WorldStateUiView {
            ui_state: 1,
            text: "Progress: 60".into(),
            extended_ui: "CAPTUREPOINT".into(),
            extended_ui_state: [60, 40, 0],
            ..Default::default()
        }],
    );
    assert!(!texts(&mut s).contains(&"Progress: 60".to_string()));
    assert!(
        s.eval::<bool>("return WorldStateCaptureBar1 ~= nil and WorldStateCaptureBar1:IsShown()")
            .unwrap(),
        "the capture bar was built and shown"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The bindings' own edges, read from Lua the way an addon would. Two are easy to get wrong and
/// both are the reference's (`0x4c5a40`/`0x4c5a70`): an out-of-range index answers exactly one
/// value — the number `0`, not nil and not ten nils — and a non-number argument raises.
#[test]
fn the_bindings_answer_the_reference_shape() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.set_world_state_ui(vec![WorldStateUiView {
        ui_state: 7,
        text: "Towers Controlled: 3".into(),
        icon: "Interface\\WorldStateFrame\\AllianceTower".into(),
        tooltip: "Alliance Towers Controlled".into(),
        extended_ui_state: [1, 2, 3],
        ..Default::default()
    }]);

    assert_eq!(s.eval::<i64>("return GetNumWorldStateUI()").unwrap(), 1);
    // Ten values, in order, with no nils among the strings.
    assert_eq!(
        s.eval::<String>(
            "local a,b,c,d,e,f,g,h,i,j = GetWorldStateUIInfo(1)\n\
             return a..'|'..b..'|'..c..'|'..d..'|'..e..'|'..f..'|'..g..'|'..h..'|'..i..'|'..j"
        )
        .unwrap(),
        "7|Towers Controlled: 3|Interface\\WorldStateFrame\\AllianceTower||\
         Alliance Towers Controlled|||1|2|3",
        "empty columns are empty strings, never nil"
    );
    assert_eq!(s.arity("GetWorldStateUIInfo(1)").unwrap(), 10);

    // Out of range: ONE value, the number 0.
    assert_eq!(s.arity("GetWorldStateUIInfo(2)").unwrap(), 1);
    assert_eq!(s.eval::<i64>("return GetWorldStateUIInfo(2)").unwrap(), 0);
    assert_eq!(
        s.eval::<i64>("return GetWorldStateUIInfo(0)").unwrap(),
        0,
        "the index is 1-based"
    );

    // A non-number argument raises rather than answering quietly.
    assert!(
        s.eval::<i64>("return GetWorldStateUIInfo('nope')").is_err(),
        "a non-numeric argument is an error, not a nil"
    );
    // ...but a numeric string coerces, as `lua_isnumber` does.
    assert_eq!(
        s.eval::<i64>("return (GetWorldStateUIInfo('1'))").unwrap(),
        7
    );
}

/// The alpha bounding box of a shipped icon, in texel coordinates (`x0, y0, x1, y1`, y down).
fn ink_box(chain: &mut benilla_formats::Chain, path: &str) -> (f32, f32, f32, f32, f32, f32) {
    let bytes = chain.read_file(path).expect("read icon");
    let (w, h, rgba) = benilla_formats::blp_to_rgba(&bytes).expect("decode icon");
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0u32, 0u32);
    for y in 0..h {
        for x in 0..w {
            if rgba[((y * w + x) * 4 + 3) as usize] > 8 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    (
        x0 as f32,
        y0 as f32,
        (x1 + 1) as f32,
        (y1 + 1) as f32,
        w as f32,
        h as f32,
    )
}

/// **Where the ink actually lands** — the test the first pass of this frame needed and did not
/// have. Every icon the readout names is a sprite authored into the UPPER-LEFT corner of a
/// power-of-two canvas (`AllianceTower` fills 16x16 of 32x32; `UI-PVP-Alliance` ~40x40 of 64x64),
/// nothing is cropped, and the reference compensates *geometrically* — a 42x42 slot hung 6 units
/// off the row's left edge with the label seated 10 above its centreline (`WorldStateFrame.xml`).
/// Pin the OUTCOME rather than the constants: whatever the numbers, the visible art must sit beside
/// its label and share its line. A snug icon box — the obvious thing to write, and what we shipped
/// — puts the ink up and to the left of its own slot, which is exactly what the director saw.
/// Skips without client data.
#[test]
fn the_visible_ink_sits_beside_its_label_not_adrift_of_it() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("chain");

    for (icon, label) in [
        (
            "Interface\\WorldStateFrame\\AllianceTower",
            "Towers Controlled: 3",
        ),
        ("Interface\\TargetingFrame\\UI-PVP-Alliance", "0/200"),
    ] {
        let mut s = harness();
        s.fire_event("PLAYER_ENTERING_WORLD", vec![ScriptValue::Str("".into())]);
        push(&mut s, vec![row(icon, label, "tip")]);
        // The label is auto-sized, so its box only exists once the host has measured it — the app
        // measures every frame; an unmeasured FontString has no height of its own and falls back to
        // its owner's, which would put the label on the row's centreline rather than its anchor's.
        s.resolve();
        let answers: Vec<(u32, f32, f32, u64)> = s
            .fontstrings_needing_measure()
            .into_iter()
            .map(|r| (r.id, r.text.chars().count() as f32 * 5.0, 10.0, r.key))
            .collect();
        s.set_measured_text_unwrapped(&answers);
        s.resolve();

        let quads = s.extract();
        let slot = quads
            .iter()
            .find(|q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p == icon))
            .and_then(|q| q.rect)
            .expect("the icon slot resolved");
        let text = quads
            .iter()
            .find(|q| matches!(&q.content, QuadContent::Text { text: Some(t), .. } if t == label))
            .and_then(|q| q.rect)
            .expect("the label resolved");

        // The whole canvas is drawn into the slot (no texcoords anywhere on these regions), so the
        // ink's screen rect is its texel box scaled into it. Rects are y-UP.
        let (_, iy0, ix1, iy1, tw, th) = ink_box(&mut chain, &format!("{icon}.blp"));
        let sw = (slot.right - slot.left) / tw;
        let sh = (slot.top - slot.bottom) / th;
        let ink_right = slot.left + ix1 * sw;
        let ink_mid_y = slot.top - (iy0 + iy1) * 0.5 * sh;
        let label_mid_y = (text.top + text.bottom) * 0.5;

        let (row_top, row_bottom) = s
            .eval::<(f64, f64)>("local r = AlwaysUpFrame1 return r:GetTop(), r:GetBottom()")
            .expect("the row resolved");
        let row_h = (row_top - row_bottom) as f32;

        // 1 · The art reads at the row's scale. This is the one that bites: squeeze the whole
        //     canvas into a snug box and the 16-texel sprite inside a 32-texel file draws at half
        //     size — a speck beside its label, which is what the oversized slot exists to prevent.
        let ink_h = (iy1 - iy0) * sh;
        assert!(
            ink_h >= 0.7 * row_h,
            "{icon}: the visible art is {ink_h} tall in a {row_h} row — the slot is sized for the \
             canvas, not for the sprite inside it"
        );
        // 2 · It ends just before its label rather than half a slot away.
        let gap = text.left - ink_right;
        assert!(
            (0.0..=10.0).contains(&gap),
            "{icon}: the ink must end just before its label — gap {gap}"
        );
        // 3 · And the two share a line.
        assert!(
            (ink_mid_y - label_mid_y).abs() <= 4.0,
            "{icon}: the ink and its label must share a line — ink centre {ink_mid_y}, \
             label centre {label_mid_y}"
        );
    }
}
