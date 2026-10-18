//! The argument forms of the widget texture setters (`SetStatusBarTexture` `0x78f870`,
//! `SetThumbTexture` `0x7902b0`, the four ColorSelect setters `0x78de90`, `0x78e160`, `0x78e450`,
//! `0x78e720`), and the bar texture's texcoord flag (`0x7831f0`). Each forks on `lua_type`
//! (`0x6f3400`) and `lua_isstring` (`0x6f3510`), which takes a string or a number: a Texture object
//! is installed, a string or number is loaded as a path, nil clears, anything else, no argument
//! included, raises the usage. There is no colour form.

use super::common::script;
use crate::script::*;

fn err(s: &UiScript, code: &str) -> String {
    s.run(code).unwrap_err().to_string()
}

fn xml(s: &UiScript, text: &str) {
    let doc = crate::framexml::parse(text).expect("valid FrameXML");
    let report = crate::loader::load(s, &doc, &|_: &str| None);
    assert!(report.errors.is_empty(), "errors: {:?}", report.errors);
}

/// `0x7831f0` writes `[texture+0x124] = 1` (`0x783220`) after it links the texture in, on the path
/// leg (`0x7831cb`), the object leg (`0x78fa27`) and `LoadXML`'s (`0x782f63`);
/// `GetTexCoordModifiesRect` (`0x79c120`) reads it.
#[test]
fn a_bar_texture_answers_one_to_get_tex_coord_modifies_rect() {
    let s = script();
    xml(
        &s,
        r#"<Ui>
            <StatusBar name="XmlFileBar"><BarTexture file="Interface\Bar"/></StatusBar>
            <StatusBar name="XmlColorBar"><BarTexture><Color r="1" g="0" b="0"/></BarTexture></StatusBar>
        </Ui>"#,
    );
    s.run(
        r#"
        assert(XmlFileBar:GetStatusBarTexture():GetTexCoordModifiesRect() == 1, "xml file")
        assert(XmlColorBar:GetStatusBarTexture():GetTexCoordModifiesRect() == 1, "xml colour")

        local p = CreateFrame("StatusBar", "PathBar")
        p:SetStatusBarTexture("Interface\\Bar")
        assert(p:GetStatusBarTexture():GetTexCoordModifiesRect() == 1, "path")

        local o = CreateFrame("StatusBar", "ObjBar")
        local t = o:CreateTexture(nil, "ARTWORK")
        assert(t:GetTexCoordModifiesRect() == nil, "a fresh texture")
        o:SetStatusBarTexture(t)
        assert(t:GetTexCoordModifiesRect() == 1, "object")

        -- A bar that exists takes the path leg's SetTexture alone (`0x783140`), so what a script
        -- wrote to the flag stays.
        t:SetTexCoordModifiesRect(nil)
        o:SetStatusBarTexture("Interface\\Other")
        assert(o:GetStatusBarTexture() == t and t:GetTexCoordModifiesRect() == nil, "reused")
        -- A displaced texture is gone; its replacement is a new object and gets the flag.
        local u = o:CreateTexture(nil, "ARTWORK")
        o:SetStatusBarTexture(u)
        assert(u:GetTexCoordModifiesRect() == 1, "replacement")
    "#,
    )
    .unwrap();
}

/// `lua_isstring` accepts a number and `lua_tostring` converts it (`0x78fa35`..`0x78fa51`), so
/// `1` is the file `"1"`, and the `(r, g, b)` form of an older client is the path `"0.2"` with the
/// two other numbers read as a layer name that is none. `GetTexture` answers a path without its
/// extension, so `"0.2"` reads `"0"`; the quad is what shows the whole file name.
#[test]
fn a_number_loads_as_its_own_path() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local sb = CreateFrame("StatusBar", "NumBar")
        sb:SetPoint("CENTER", 0, 0) sb:SetWidth(50) sb:SetHeight(50)
        sb:SetMinMaxValues(0, 1) sb:SetValue(1)
        sb:SetStatusBarTexture(0.2, 0.7, 0.2)
        assert(sb:GetStatusBarTexture():GetTexture() ~= "Solid Texture", "no solid fill")
        local sl = CreateFrame("Slider", "NumSlider")
        sl:SetThumbTexture(2, 0.5, 0.5, 1)
        assert(sl:GetThumbTexture():GetTexture() == "2")

        local cs = CreateFrame("ColorSelect", "NumCS")
        cs:SetColorWheelThumbTexture(3)
        cs:SetColorValueThumbTexture(4)
        assert(cs:GetColorWheelThumbTexture():GetTexture() == "3")
        assert(cs:GetColorValueThumbTexture():GetTexture() == "4")

        local one = CreateFrame("StatusBar", "NumBarOne")
        one:SetStatusBarTexture(1)
        assert(one:GetStatusBarTexture():GetTexture() == "1")
    "#,
    )
    .unwrap();
    s.resolve();
    let paths: Vec<String> = s
        .extract()
        .iter()
        .filter_map(|q| match &q.content {
            QuadContent::Texture {
                path: Some(p),
                color: None,
                ..
            } => Some(p.clone()),
            _ => None,
        })
        .collect();
    assert!(paths.contains(&"0.2".to_string()), "{paths:?}");
}

/// No argument is not nil (`0x6f3400` answers -1), and any other type is no texture either: the
/// usage line (`0x87b1a0`, `0x87b2f0`, `0x87adc0`, `0x87af40`, `0x87ad10`, `0x87ae90`) with the
/// frame's name, and nothing changes.
#[test]
fn no_argument_and_other_types_raise_the_usage() {
    let s = script();
    s.run(
        r#"
        sb = CreateFrame("StatusBar", "UseBar")
        sl = CreateFrame("Slider", "UseSlider")
        cs = CreateFrame("ColorSelect", "UseCS")
        sb:SetStatusBarTexture("keep")
        sl:SetThumbTexture("keep")
    "#,
    )
    .unwrap();
    for (call, usage) in [
        (
            "sb:SetStatusBarTexture()",
            r#"Usage: UseBar:SetStatusBarTexture(texture or "texture" or nil [, "layer"])"#,
        ),
        (
            "sb:SetStatusBarTexture(true)",
            r#"Usage: UseBar:SetStatusBarTexture(texture or "texture" or nil [, "layer"])"#,
        ),
        (
            "sl:SetThumbTexture()",
            r#"Usage: UseSlider:SetThumbTexture(texture or "texture" or nil)"#,
        ),
        (
            "sl:SetThumbTexture(function() end)",
            r#"Usage: UseSlider:SetThumbTexture(texture or "texture" or nil)"#,
        ),
        (
            "cs:SetColorWheelThumbTexture()",
            r#"Usage: UseCS:SetColorWheelThumbTexture(texture or "texture" or nil)"#,
        ),
        (
            "cs:SetColorValueThumbTexture(false)",
            r#"Usage: UseCS:SetColorValueThumbTexture(texture or "texture" or nil)"#,
        ),
        (
            "cs:SetColorWheelTexture()",
            "Usage: UseCS:SetColorWheelTexture(texture or nil)",
        ),
        (
            "cs:SetColorValueTexture()",
            "Usage: UseCS:SetColorValueTexture(texture or nil)",
        ),
    ] {
        let e = err(&s, call);
        assert!(e.contains(usage), "`{call}` -> {e}");
    }
    s.run(
        r#"
        assert(sb:GetStatusBarTexture():GetTexture() == "keep")
        assert(sl:GetThumbTexture():GetTexture() == "keep")
        assert(cs:GetColorWheelTexture() == nil and cs:GetColorWheelThumbTexture() == nil)
    "#,
    )
    .unwrap();
}

/// The wheel and the strip have no path leg (`0x87ad10`, `0x87ae90`): a string or a number raises,
/// a Texture object installs and nil clears.
#[test]
fn the_wheel_and_the_strip_take_a_texture_or_nil_only() {
    let s = script();
    s.run(
        r#"
        cs = CreateFrame("ColorSelect", "WheelCS")
        t = cs:CreateTexture(nil, "ARTWORK")
        cs:SetColorWheelTexture(t)
        assert(cs:GetColorWheelTexture() == t)
        cs:SetColorWheelTexture(nil)
        assert(cs:GetColorWheelTexture() == nil)
        t = cs:CreateTexture(nil, "ARTWORK")
        cs:SetColorValueTexture(t)
        assert(cs:GetColorValueTexture() == t)
    "#,
    )
    .unwrap();
    for call in [
        r#"cs:SetColorWheelTexture("Interface\\Wheel")"#,
        "cs:SetColorWheelTexture(1)",
        r#"cs:SetColorValueTexture("Interface\\Strip")"#,
    ] {
        let e = err(&s, call);
        assert!(e.contains("Usage: WheelCS:SetColor"), "`{call}` -> {e}");
    }
    s.run("assert(cs:GetColorValueTexture() == t)").unwrap();
}

/// The thumb setters read no layer (`0x7902b0` has no layer argument): a second argument is
/// ignored, and the thumb is OVERLAY. The bar reads one (`0x78f8fb`) for a bar it creates, and an
/// existing bar keeps its layer (`0x783140` never reads it).
#[test]
fn the_thumbs_take_no_layer_and_an_existing_bar_keeps_its_own() {
    let s = script();
    s.run(
        r#"
        local sl = CreateFrame("Slider", "LayerSlider")
        sl:SetThumbTexture("p", "BACKGROUND")
        assert(sl:GetThumbTexture():GetDrawLayer() == "OVERLAY", sl:GetThumbTexture():GetDrawLayer())

        local sb = CreateFrame("StatusBar", "LayerBar")
        sb:SetStatusBarTexture("a", "BORDER")
        assert(sb:GetStatusBarTexture():GetDrawLayer() == "BORDER")
        sb:SetStatusBarTexture("b", "HIGHLIGHT")
        assert(sb:GetStatusBarTexture():GetDrawLayer() == "BORDER", "an existing bar keeps its layer")
        assert(sb:GetStatusBarTexture():GetTexture() == "b")
    "#,
    )
    .unwrap();
}

/// What the loader reaches with the colour form the Lua setters no longer have: a `<Color>` in
/// `<BarTexture>` or `<ThumbTexture>` is the texture's own fill, and a file-less ColorSelect
/// element still creates its region.
#[test]
fn the_loader_still_fills_and_creates_what_the_setters_no_longer_take() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    xml(
        &s,
        r#"<Ui>
            <StatusBar name="FillBar" minValue="0" maxValue="100" defaultValue="100">
                <Size><AbsDimension x="100" y="10"/></Size>
                <Anchors><Anchor point="BOTTOMLEFT"/></Anchors>
                <BarTexture><Color r="0.25" g="0.5" b="0.75" a="1"/></BarTexture>
            </StatusBar>
            <Slider name="FillSlider">
                <Size><AbsDimension x="10" y="100"/></Size>
                <Anchors><Anchor point="TOPLEFT"/></Anchors>
                <ThumbTexture name="FillThumb"><Size><AbsDimension x="10" y="10"/></Size><Color r="0.75" g="0.5" b="0.25" a="1"/></ThumbTexture>
            </Slider>
            <ColorSelect name="BareCS">
                <Size><AbsDimension x="100" y="100"/></Size>
                <Anchors><Anchor point="CENTER"/></Anchors>
                <ColorWheelTexture/>
                <ColorWheelThumbTexture/>
                <ColorValueTexture/>
                <ColorValueThumbTexture/>
            </ColorSelect>
        </Ui>"#,
    );
    s.run(
        r#"
        assert(FillBar:GetStatusBarTexture() ~= nil and FillSlider:GetThumbTexture() ~= nil)
        assert(BareCS:GetColorWheelTexture() ~= nil and BareCS:GetColorWheelThumbTexture() ~= nil)
        assert(BareCS:GetColorValueTexture() ~= nil and BareCS:GetColorValueThumbTexture() ~= nil)
        assert(BareCS:GetColorWheelTexture():GetTexture() == nil)
    "#,
    )
    .unwrap();
    s.resolve();
    let fills: Vec<[f32; 4]> = s
        .extract()
        .iter()
        .filter_map(|q| match &q.content {
            QuadContent::Texture {
                path: None,
                color: Some(c),
                ..
            } => Some(*c),
            _ => None,
        })
        .collect();
    assert!(
        fills.contains(&crate::color::quant::drawn([0.25, 0.5, 0.75, 1.0])),
        "{fills:?}"
    );
    assert!(
        fills.contains(&crate::color::quant::drawn([0.75, 0.5, 0.25, 1.0])),
        "{fills:?}"
    );
}

/// The bar's path leg gives a new bar texture all points on its bar (`0x7831bf`), as the XML leg's
/// post-step does (`0x7701c0`); a texture handed in as an object keeps what it has.
#[test]
fn an_installed_bar_texture_has_all_points_on_its_bar() {
    let s = script();
    xml(
        &s,
        r#"<Ui>
            <StatusBar name="PtsXmlBar"><BarTexture file="Interface\Bar"/></StatusBar>
        </Ui>"#,
    );
    s.run(
        r#"
        local function onbar(t, bar)
            assert(t:GetNumPoints() == 2, "points: " .. t:GetNumPoints())
            local _, rel = t:GetPoint(1)
            assert(rel == bar, "relative to the bar")
        end
        onbar(PtsXmlBar:GetStatusBarTexture(), PtsXmlBar)

        local p = CreateFrame("StatusBar", "PtsPathBar")
        p:SetStatusBarTexture("Interface\\Bar")
        onbar(p:GetStatusBarTexture(), p)

        local o = CreateFrame("StatusBar", "PtsObjBar")
        local t = o:CreateTexture(nil, "ARTWORK")
        assert(t:GetNumPoints() == 0)
        o:SetStatusBarTexture(t)
        assert(t:GetNumPoints() == 0, "an object keeps what it has")
    "#,
    )
    .unwrap();
}

/// An empty `<BarTexture/>` or `<ThumbTexture/>` still installs the texture the texture adder
/// builds (`0x782f63`, `0x7895f6`), and draws nothing.
#[test]
fn an_empty_bar_or_thumb_element_installs_a_texture() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    xml(
        &s,
        r#"<Ui>
            <StatusBar name="EmptyBar"><Size><AbsDimension x="50" y="10"/></Size>
                <Anchors><Anchor point="CENTER"/></Anchors><BarTexture/></StatusBar>
            <Slider name="EmptySlider"><Size><AbsDimension x="10" y="50"/></Size>
                <Anchors><Anchor point="TOPLEFT"/></Anchors><ThumbTexture/></Slider>
        </Ui>"#,
    );
    s.run(
        r#"
        assert(EmptyBar:GetStatusBarTexture() ~= nil, "bar")
        assert(EmptyBar:GetStatusBarTexture():GetTexCoordModifiesRect() == 1)
        assert(EmptySlider:GetThumbTexture() ~= nil, "thumb")
    "#,
    )
    .unwrap();
    s.resolve();
    let textured = s
        .extract()
        .iter()
        .filter(|q| {
            matches!(
                q.content,
                QuadContent::Texture { path: Some(_), .. }
                    | QuadContent::Texture { color: Some(_), .. }
            )
        })
        .count();
    assert_eq!(textured, 0, "an empty texture draws nothing");
}

/// `SetStatusBarColor` (`0x78fc20` → `0x782dd0`) tints the bar only if one exists and keeps
/// nothing otherwise: no bar, no effect, and a bar installed later is untinted.
#[test]
fn set_status_bar_color_with_no_bar_does_nothing() {
    let s = script();
    // 255 read back times the float at `0x81c6b4`.
    s.lua()
        .globals()
        .set("ONE", crate::color::channel(255))
        .unwrap();
    s.run(
        r#"
        local sb = CreateFrame("StatusBar", "TintBar")
        sb:SetStatusBarColor(1, 0, 0)
        assert(sb:GetStatusBarTexture() == nil, "no bar made")
        sb:SetStatusBarTexture("p")
        local r, g, b, a = sb:GetStatusBarColor()
        assert(r == ONE and g == ONE and b == ONE and a == ONE, "untinted: " .. r .. g .. b)
        sb:SetStatusBarColor(0, 1, 0)
        r, g, b = sb:GetStatusBarColor()
        assert(r == 0 and g == ONE and b == 0, "tinted once there is a bar")
    "#,
    )
    .unwrap();
}

/// `<BarTexture>` and `<ThumbTexture>` go through the texture adder (`0x6f26f0`): a named one is
/// published under its expanded name, and its `<TexCoords>` and `alphaMode` apply.
#[test]
fn a_named_bar_texture_is_published_and_keeps_its_texcoords() {
    let s = script();
    xml(
        &s,
        r#"<Ui>
            <StatusBar name="NamedBarFrame">
                <BarTexture name="$parentBar" file="Interface\Bar" alphaMode="ADD">
                    <TexCoords left="0.1" right="0.9" top="0.2" bottom="0.8"/>
                </BarTexture>
            </StatusBar>
            <Slider name="NamedSliderFrame">
                <ThumbTexture name="$parentThumb" file="Interface\Knob">
                    <TexCoords left="0.25" right="0.75" top="0" bottom="1"/>
                </ThumbTexture>
            </Slider>
        </Ui>"#,
    );
    s.run(
        r#"
        assert(NamedBarFrameBar == NamedBarFrame:GetStatusBarTexture(), "published")
        assert(NamedBarFrameBar:GetName() == "NamedBarFrameBar")
        local ulx, uly = NamedBarFrameBar:GetTexCoord()
        assert(math.abs(ulx - 0.1) < 1e-5 and math.abs(uly - 0.2) < 1e-5, ulx .. " " .. uly)
        assert(NamedBarFrameBar:GetBlendMode() == "ADD")
        assert(NamedSliderFrameThumb == NamedSliderFrame:GetThumbTexture(), "thumb published")
        ulx = NamedSliderFrameThumb:GetTexCoord()
        assert(math.abs(ulx - 0.25) < 1e-5)
    "#,
    )
    .unwrap();
}
