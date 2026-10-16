//! The text setters that call `0x771d80(text, 1)` run the number-separator pass `0x771c50` while
//! `SetEuropeanNumbers` holds (`0x771daa`): GameTooltip lines, both message frames, SimpleHTML,
//! XML `text=`, chat bubbles and nameplates. An EditBox passes 0 (`0x77d850`) and keeps its text.
//! Each test sets the switch and writes `1.5`, which reads back `1,5`; with it off, as it was.

use super::common::script;
use crate::script::{
    BubbleGeometry, BubbleState, MeasureRequest, Model, PlateGeometry, PlateState, QuadContent,
    TextMeasure, UiScript,
};

fn european() -> UiScript {
    let mut s = script();
    s.set_screen_size(1024.0, 768.0);
    s.set_european_numbers(true);
    // Stock `Fonts.xml` sets it; a plate or bubble built without it has no font and draws no text.
    s.run(r#"NAMEPLATE_FONT = "Fonts\\FRIZQT__.TTF""#).unwrap();
    s
}

/// A tooltip's lines: `AddLine` and `SetText` (`0x531df5` → `AddLine 0x5303b0` → `0x5302e7`),
/// `AddDoubleLine`'s two sides (`0x5302e7`, `0x53033d`) and `AppendText`, whose helper `0x5305b0`
/// formats the line's text again and stores it through the same setter (`0x53060e`).
#[test]
fn tooltip_lines_swap_their_separators() {
    let mut s = european();
    s.run(
        r#"
        local owner = CreateFrame("Button", "Slot")
        owner:SetPoint("TOPLEFT", 100, -100); owner:SetWidth(40); owner:SetHeight(40)
        tt = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        tt:SetOwner(owner, "ANCHOR_RIGHT")
        tt:AddLine("Deals 1.5 damage")
        tt:AddDoubleLine("Range 2.5", "Cost 3.5")
    "#,
    )
    .unwrap();
    let read = |s: &UiScript, name: &str| -> String {
        s.eval(&format!("return {name}:GetText()")).unwrap()
    };
    assert_eq!(read(&s, "TTTextLeft1"), "Deals 1,5 damage");
    assert_eq!(read(&s, "TTTextLeft2"), "Range 2,5");
    assert_eq!(read(&s, "TTTextRight2"), "Cost 3,5");

    s.run(r#"tt:SetText("Takes 4.5") tt:AppendText(" or 5.5")"#)
        .unwrap();
    assert_eq!(read(&s, "TTTextLeft1"), "Takes 4,5 or 5,5");

    s.set_european_numbers(false);
    s.run(r#"tt:SetText("Takes 6.5")"#).unwrap();
    assert_eq!(read(&s, "TTTextLeft1"), "Takes 6.5", "off: as given");
}

/// `AddMessage` on a MessageFrame (`0x7863fe`) swaps as the line is added; a ScrollingMessageFrame
/// swaps each time it shows its lines (`ShowLine`, `0x788b3c`), from the raw text it keeps, so its
/// lines read the switch as it stands and the app's chat feed (which stands for the verb) too.
#[test]
fn message_frame_lines_swap_their_separators() {
    let mut s = script();
    s.set_screen_size(1024.0, 768.0);
    s.run(
        r#"
        local up = CreateFrame("Frame", "UIParent")
        up:SetPoint("BOTTOMLEFT", 0, 0); up:SetPoint("TOPRIGHT", 0, 0)
        for i, name in ipairs({"MF", "SMF"}) do
            local f = CreateFrame(i == 1 and "MessageFrame" or "ScrollingMessageFrame", name, up)
            f:SetPoint("BOTTOMLEFT", 0, 100 * i); f:SetWidth(300); f:SetHeight(80)
            f:SetFont("Fonts\\FRIZQT__.TTF", 12)
        end
    "#,
    )
    .unwrap();
    let texts = |s: &mut UiScript| -> Vec<String> {
        s.resolve();
        let mut t: Vec<String> = s
            .extract()
            .iter()
            .filter_map(|q| match &q.content {
                QuadContent::Text { text: Some(t), .. } => Some(t.clone()),
                _ => None,
            })
            .collect();
        t.sort();
        t
    };
    // Off at boot: lines read as given.
    s.run(r#"MF:AddMessage("Off 1.5") SMF:AddMessage("Off 2.5")"#)
        .unwrap();
    assert_eq!(texts(&mut s), ["Off 1.5", "Off 2.5"]);

    // On: the scrolling frame's lines swap at their next show, the plain frame's stay as added.
    s.set_european_numbers(true);
    s.run(r#"MF:AddMessage("On 3.5")"#).unwrap();
    assert!(s.add_chat_message("SMF", "Feed 4.5", 1.0, 1.0, 1.0));
    assert_eq!(texts(&mut s), ["Feed 4,5", "Off 1.5", "Off 2,5", "On 3,5"]);

    // Off again: the scrolling frame reads as given, the plain frame keeps what it swapped.
    s.set_european_numbers(false);
    assert_eq!(texts(&mut s), ["Feed 4.5", "Off 1.5", "Off 2.5", "On 3,5"]);
}

/// SimpleHTML builds each text block through `0x771d80(text, 1)` (`0x78aea1`).
#[test]
fn simple_html_blocks_swap_their_separators() {
    let s = european();
    s.run(
        r#"
        local h = CreateFrame("SimpleHTML", "H")
        h:SetWidth(200); h:SetHeight(200)
        h:SetText("<html><body><h1>Chapter 1.5</h1><p>Page 2.5</p></body></html>")
    "#,
    )
    .unwrap();
    let model = s.lua().app_data_ref::<Model>().expect("model");
    let fh = model.arena.lookup("H").expect("SimpleHTML frame");
    let texts: Vec<String> = model.simple_html[&fh]
        .blocks
        .iter()
        .filter_map(|rh| model.region_data.get(rh).and_then(|d| d.text.clone()))
        .collect();
    assert_eq!(texts, ["Chapter 1,5", "Page 2,5"]);
}

/// XML `text=` on a FontString (`0x77103a`) and on a Button's label reach `SetText` with the flag.
/// A guard, not a fix: the loader stores through the string's own setter, which swaps.
#[test]
fn xml_text_attributes_swap_their_separators() {
    let s = european();
    let doc = crate::framexml::parse(
        r#"<Ui><Frame name="XmlOwner">
            <Layers><Layer level="ARTWORK"><FontString name="XmlText" text="Ratio 1.5"/></Layer></Layers>
            <Frames><Button name="XmlButton" text="Cost 2.5"/></Frames>
        </Frame></Ui>"#,
    )
    .expect("valid FrameXML");
    let report = crate::loader::load(&s, &doc, &|_| None);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(
        s.eval::<(String, String)>("return XmlText:GetText(), XmlButton:GetText()")
            .unwrap(),
        ("Ratio 1,5".to_string(), "Cost 2,5".to_string())
    );
}

/// An EditBox's text passes 0 for the flag (`0x77d850`): the switch leaves it alone.
#[test]
fn an_edit_box_keeps_its_separators() {
    let s = european();
    s.run(r#"local e = CreateFrame("EditBox", "E") e:SetText("1.5") Typed = e:GetText()"#)
        .unwrap();
    assert_eq!(s.eval::<String>("return Typed").unwrap(), "1.5");
}

struct BlockFont;

impl TextMeasure for BlockFont {
    fn measure(&mut self, req: &MeasureRequest) -> (f32, f32, f32) {
        let natural = req.text.chars().count() as f32 * 6.0;
        (natural, 13.0, natural)
    }
}

/// A chat bubble's line (`0x4b16c0`).
#[test]
fn a_chat_bubble_line_swaps_its_separators() {
    let mut s = european();
    s.set_text_measurer(Box::new(BlockFont));
    s.run(r#"WorldFrame = CreateFrame("WorldFrame", "WorldFrame") WorldFrame:SetAllPoints()"#)
        .unwrap();
    s.resolve();
    let geometry = BubbleGeometry {
        border: 16.0,
        margin: 13.0,
        wrap_cap: 256.0,
        text_height: 13.0,
        device_px: 1.0,
    };
    let line = BubbleState {
        key: 7,
        line: 1,
        text: "I have 1.5 gold".into(),
        colour: [1.0, 1.0, 1.0],
        shown: true,
        alpha: 1.0,
        seat: (500.0, 300.0),
    };
    s.sync_chat_bubbles(geometry, &[line]);
    s.resolve();
    assert_eq!(
        s.eval::<String>(
            "local regions = { (WorldFrame:GetChildren()):GetRegions() } return regions[11]:GetText()"
        )
        .unwrap(),
        "I have 1,5 gold"
    );
}

/// A nameplate's name (`0x7cb718`) and level (`0x7cbcca`).
#[test]
fn a_nameplate_name_and_level_swap_their_separators() {
    let mut s = european();
    s.run(r#"WorldFrame = CreateFrame("WorldFrame", "WorldFrame") WorldFrame:SetAllPoints()"#)
        .unwrap();
    s.resolve();
    let geometry = PlateGeometry {
        width: 128.0,
        height: 32.0,
        bar_off_x: 4.0,
        bar_off_y: 4.0,
        bar_width: 103.0,
        bar_height: 9.0,
        level_off_x: 11.8,
        level_off_y: 9.1,
        skull_size: 12.8,
        raid_size: 25.6,
        name_height: 12.8,
        level_height: 11.0,
        shadow_offset: 1.0,
    };
    let state = PlateState {
        key: 1,
        top_centre: (500.0, 400.0),
        health: 30.0,
        max_health: 40.0,
        bar_colour: [1.0, 0.0, 0.0],
        name: "Wolf 1.5".into(),
        level: Some(12),
        skull: false,
        level_colour: [1.0, 1.0, 0.0],
        raid_icon: None,
        alpha: 1.0,
        lit: false,
        hovered: false,
    };
    s.sync_nameplates(geometry, &[state]);
    // A new plate draws from the first UI update after its sync, as the other plate tests drive.
    s.tick(0.0);
    s.resolve();
    let texts: Vec<String> = s
        .extract()
        .iter()
        .filter_map(|q| match &q.content {
            QuadContent::Text { text: Some(t), .. } => Some(t.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, ["Wolf 1,5", "12"]);
}
