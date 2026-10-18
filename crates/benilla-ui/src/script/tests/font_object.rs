//! Font objects as first-class Lua objects ([`crate::script::font`]), driven as addons drive them:
//! the bare global, the no-argument `CreateFontString`, `CreateFont`.

use super::common::script;
use crate::script::{FontObject, JustifyH, JustifyV, Outline, QuadContent, UiScript};

/// Load a FrameXML fragment through the real loader, asserting it reported no errors.
fn load(s: &UiScript, xml: &str) {
    let doc = crate::framexml::parse(xml).expect("valid FrameXML");
    let report = crate::loader::load(s, &doc, &|_| None);
    assert!(
        report.errors.is_empty(),
        "loader errors: {:?}",
        report.errors
    );
}

/// The resolved `(face, height, colour)` of the one text quad on screen.
fn painted(s: &UiScript, text: &str) -> (Option<String>, Option<f32>, Option<[f32; 4]>) {
    s.extract()
        .into_iter()
        .find_map(|q| match q.content {
            QuadContent::Text {
                text: Some(ref t),
                ref font,
                font_height,
                color,
                ..
            } if t == text => Some((font.clone(), font_height, color)),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no text quad reading {text:?}"))
}

// ── Publication ──

#[test]
fn a_declared_font_is_published_as_a_global_object() {
    let s = script();
    load(
        &s,
        r#"<Ui>
             <Font name="GameFontNormal" font="Fonts\FRIZQT__.TTF" justifyH="LEFT">
               <FontHeight><AbsValue val="12"/></FontHeight>
               <Color r="1" g="0.82" b="0"/>
               <Shadow><Offset><AbsDimension x="1" y="-1"/></Offset><Color r="0" g="0" b="0"/></Shadow>
             </Font>
           </Ui>"#,
    );
    assert_eq!(
        s.eval::<String>("return GameFontNormal:GetObjectType()")
            .unwrap(),
        "Font"
    );
    assert!(s
        .eval::<bool>("return GameFontNormal:IsObjectType('Font')")
        .unwrap());
    assert_eq!(
        s.eval::<String>("return GameFontNormal:GetName()").unwrap(),
        "GameFontNormal"
    );
    // Two reads of the global are one object, which `fs:GetFontObject() == GameFontNormal` needs.
    assert!(s
        .eval::<bool>("return GameFontNormal == GameFontNormal")
        .unwrap());

    assert_eq!(
        s.eval::<(String, f32, String)>("return GameFontNormal:GetFont()")
            .unwrap(),
        ("Fonts\\FRIZQT__.TTF".to_string(), 12.0, String::new())
    );
    let (r, g, b, _) = s
        .eval::<(f32, f32, f32, f32)>("return GameFontNormal:GetTextColor()")
        .unwrap();
    let read = crate::color::quant::read32;
    assert_eq!((r, g, b), (read(1.0), read(0.82), 0.0));
    assert_eq!(
        s.eval::<(f32, f32)>("return GameFontNormal:GetShadowOffset()")
            .unwrap(),
        (1.0, -1.0)
    );
    assert_eq!(
        s.eval::<(f32, f32, f32, f32)>("return GameFontNormal:GetShadowColor()")
            .unwrap(),
        (0.0, 0.0, 0.0, read(1.0))
    );
    assert_eq!(
        s.eval::<String>("return GameFontNormal:GetJustifyH()")
            .unwrap(),
        "LEFT"
    );
}

/// The header-size read at `Tablet-2.0.lua:289`, verbatim.
#[test]
fn tablet_reads_the_tooltip_header_size_off_the_global() {
    let s = script();
    load(
        &s,
        r#"<Ui>
             <Font name="GameTooltipText" font="Fonts\FRIZQT__.TTF">
               <FontHeight><AbsValue val="12"/></FontHeight>
             </Font>
             <Font name="GameTooltipHeaderText" inherits="GameTooltipText" font="Fonts\FRIZQT__.TTF">
               <FontHeight><AbsValue val="14"/></FontHeight>
             </Font>
           </Ui>"#,
    );
    let (header, normal) = s
        .eval::<(f32, f32)>(
            r#"
            local headerSize, normalSize
            if GameTooltipHeaderText then
                _, headerSize = GameTooltipHeaderText:GetFont()
            else
                headerSize = 14
            end
            if GameTooltipText then
                _, normalSize = GameTooltipText:GetFont()
            else
                normalSize = 12
            end
            return headerSize, normalSize
        "#,
        )
        .unwrap();
    assert_eq!((header, normal), (14.0, 12.0));
}

/// `inherits=` links the font to its parent (`0x783ce6`), and the published object's block holds
/// the parent's groups merged under its own.
#[test]
fn an_inheriting_font_holds_its_parents_groups_under_its_own() {
    let s = script();
    load(
        &s,
        r#"<Ui>
             <Font name="MasterFont" font="Fonts\FRIZQT__.TTF" justifyH="CENTER">
               <FontHeight><AbsValue val="10"/></FontHeight>
               <Color r="1" g="1" b="1"/>
               <Shadow><Offset><AbsDimension x="1" y="-1"/></Offset><Color r="0" g="0" b="0"/></Shadow>
             </Font>
             <Font name="DerivedFont" inherits="MasterFont" font="Fonts\FRIZQT__.TTF" outline="NORMAL">
               <FontHeight><AbsValue val="18"/></FontHeight>
             </Font>
           </Ui>"#,
    );
    let derived = s.font_object("DerivedFont").expect("registered");
    assert_eq!(derived.font.as_deref(), Some("Fonts\\FRIZQT__.TTF"));
    assert_eq!(derived.height, Some(18.0));
    assert_eq!(
        derived.color,
        Some(crate::color::Rgba8::from_f32([1.0, 1.0, 1.0, 1.0]))
    );
    assert_eq!(derived.outline, Outline::Normal);
    assert_eq!(derived.justify.map(|j| j.name_h()), Some("CENTER"));
    assert!(derived.shadow.is_some(), "the shadow inherits too");
    assert_eq!(
        s.eval::<(String, f32, String)>("return DerivedFont:GetFont()")
            .unwrap(),
        (
            "Fonts\\FRIZQT__.TTF".to_string(),
            18.0,
            "OUTLINE".to_string()
        )
    );
}

// ── SetFontObject and GetFontObject ──

/// The tooltip build at `Gratuity-2.0.lua:47-59`: it needs a no-argument `CreateFontString`,
/// `SetFontObject` taking the object, and `AddFontStrings`.
#[test]
fn gratuity_builds_its_thirty_line_scan_tooltip() {
    let s = script();
    load(
        &s,
        r#"<Ui>
             <Font name="GameFontNormal" font="Fonts\FRIZQT__.TTF">
               <FontHeight><AbsValue val="12"/></FontHeight>
               <Color r="1" g="0.82" b="0"/>
             </Font>
           </Ui>"#,
    );
    s.run(
        r#"
        vars = { Llines = {}, Rlines = {} }
        local tt = CreateFrame("GameTooltip")
        vars.tooltip = tt
        tt:SetOwner(tt, "ANCHOR_NONE")
        for i = 1, 30 do
            vars.Llines[i], vars.Rlines[i] = tt:CreateFontString(), tt:CreateFontString()
            vars.Llines[i]:SetFontObject(GameFontNormal)
            vars.Rlines[i]:SetFontObject(GameFontNormal)
            tt:AddFontStrings(vars.Llines[i], vars.Rlines[i])
        end
    "#,
    )
    .expect("Gratuity's CreateTooltip must not raise");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    assert_eq!(
        s.eval::<(String, f32)>("return vars.Llines[7]:GetFont()")
            .unwrap(),
        ("Fonts\\FRIZQT__.TTF".to_string(), 12.0)
    );
    assert_eq!(
        s.eval::<String>("return vars.Rlines[30]:GetFontObject():GetName()")
            .unwrap(),
        "GameFontNormal"
    );
    // Gratuity's next call: `ClearLines` over the grown line stack.
    s.run("vars.tooltip:ClearLines()").unwrap();
    // Each `AddFontStrings` raised the line limit by one (`0x529940`): a bare tooltip holds the 30
    // pairs it was given, in the order given.
    s.run(
        r#"
        for i = 1, 35 do vars.tooltip:AddLine("line " .. i) end
        assert(vars.tooltip:NumLines() == 30, vars.tooltip:NumLines())
        assert(vars.Llines[1]:GetText() == "line 1" and vars.Llines[30]:GetText() == "line 30")
    "#,
    )
    .unwrap();
}

#[test]
fn set_font_object_takes_the_object_or_the_name() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.register_font_object(
        "GameFontHighlightSmall",
        FontObject {
            font: Some("Fonts\\ARIALN.TTF".into()),
            height: Some(11.0),
            color: Some(crate::color::Rgba8::from_f32([0.25, 0.5, 0.75, 1.0])),
            outline: Outline::Thick,
            monochrome: false,
            justify: None,
            shadow: None,
            spacing: None,
        },
    );
    s.run(
        r#"
        f = CreateFrame("Frame", "TwoWays")
        f:SetWidth(200); f:SetHeight(40); f:SetPoint("CENTER", 0, 0)
        byObject = f:CreateFontString(nil, "ARTWORK")
        byObject:SetFontObject(GameFontHighlightSmall)
        byObject:SetText("obj")
        byName = f:CreateFontString(nil, "ARTWORK")
        byName:SetFontObject("GameFontHighlightSmall")
        byName:SetText("str")
    "#,
    )
    .unwrap();
    s.resolve();
    assert_eq!(painted(&s, "obj"), painted(&s, "str"));
    assert_eq!(
        painted(&s, "obj"),
        (
            Some("Fonts\\ARIALN.TTF".into()),
            Some(11.0),
            Some(crate::color::quant::drawn([0.25, 0.5, 0.75, 1.0]))
        )
    );
    assert!(s
        .eval::<bool>("return byObject:GetFontObject() == byName:GetFontObject()")
        .unwrap());

    // nil, the third form in the reference's usage string (`.rdata 0x87c5cc`), unlinks the object
    // and leaves the paint.
    s.run("byName:SetFontObject(nil)").unwrap();
    assert!(s
        .eval::<bool>("return byName:GetFontObject() == nil")
        .unwrap());
    s.resolve();
    assert_eq!(
        painted(&s, "str").1,
        Some(11.0),
        "unlinking must not repaint"
    );
    assert!(s.run("byName:SetFontObject(f)").is_err());
    assert!(s.run("byName:SetFontObject('NoSuchFont')").is_err());
}

/// `GetFontObject` returns the object, not a name, which `Dewdrop-2.0.lua:2181` indexes at once.
#[test]
fn dewdrop_recolors_a_row_from_its_own_font_object() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.register_font_object(
        "GameFontHighlightSmall",
        FontObject {
            font: Some("Fonts\\FRIZQT__.TTF".into()),
            height: Some(10.0),
            color: Some(crate::color::Rgba8::from_f32([1.0, 1.0, 1.0, 1.0])),
            outline: Outline::None,
            monochrome: false,
            justify: None,
            shadow: None,
            spacing: None,
        },
    );
    s.run(
        r#"
        f = CreateFrame("Frame", "DdRow")
        f:SetWidth(120); f:SetHeight(16); f:SetPoint("CENTER", 0, 0)
        button = { text = f:CreateFontString(nil, "ARTWORK") }
        button.text:SetFontObject(GameFontHighlightSmall)
        button.text:SetText("row")
        button.text:SetTextColor(button.text:GetFontObject():GetTextColor())
    "#,
    )
    .expect("Dewdrop's row recolor must not raise");
    s.resolve();
    assert_eq!(painted(&s, "row").2, Some([1.0, 1.0, 1.0, 1.0]));
}

// ── Mutability ──

/// A font object is a live link: mutating it repaints every FontString that inherits it, except
/// what a FontString set for itself, whose setter cleared that property's inherit-mask bit.
#[test]
fn mutating_a_font_object_repaints_everything_that_inherits_it() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.register_font_object(
        "ThemeFont",
        FontObject {
            font: Some("Fonts\\FRIZQT__.TTF".into()),
            height: Some(12.0),
            color: Some(crate::color::Rgba8::from_f32([1.0, 1.0, 1.0, 1.0])),
            outline: Outline::None,
            monochrome: false,
            justify: None,
            shadow: None,
            spacing: None,
        },
    );
    s.run(
        r#"
        f = CreateFrame("Frame", "ThemeHost")
        f:SetWidth(200); f:SetHeight(60); f:SetPoint("CENTER", 0, 0)
        a = f:CreateFontString(nil, "ARTWORK"); a:SetFontObject(ThemeFont)
        a:SetText("inherited")
        b = f:CreateFontString(nil, "ARTWORK"); b:SetFontObject(ThemeFont)
        b:SetText("overridden")
        b:SetTextColor(1, 0, 0)
    "#,
    )
    .unwrap();
    s.resolve();
    assert_eq!(painted(&s, "inherited").1, Some(12.0));
    assert_eq!(painted(&s, "overridden").2, Some([1.0, 0.0, 0.0, 1.0]));

    s.run(
        r#"
        ThemeFont:SetFont("Fonts\\MORPHEUS.TTF", 20)
        ThemeFont:SetTextColor(0, 1, 0)
    "#,
    )
    .unwrap();
    s.resolve();

    assert_eq!(
        painted(&s, "inherited"),
        (
            Some("Fonts\\MORPHEUS.TTF".into()),
            Some(20.0),
            Some([0.0, 1.0, 0.0, 1.0])
        )
    );
    assert_eq!(
        painted(&s, "overridden").0,
        Some("Fonts\\MORPHEUS.TTF".into())
    );
    assert_eq!(painted(&s, "overridden").1, Some(20.0));
    assert_eq!(
        painted(&s, "overridden").2,
        Some([1.0, 0.0, 0.0, 1.0]),
        "an explicitly-set property must survive a font-object mutation"
    );

    // Severance survives a re-point: the local setter clears the reference's inheritMask bit
    // (`+0x2c`, FontString `+0xd4`) and nothing restores it.
    s.run("b:SetFontObject(ThemeFont)").unwrap();
    s.resolve();
    assert_eq!(
        painted(&s, "overridden").2,
        Some([1.0, 0.0, 0.0, 1.0]),
        "a re-point must not restore inheritance of a severed property"
    );
    // Dewdrop re-reads the colour off the object every refresh (`Dewdrop-2.0.lua:2181`).
    s.run("b:SetTextColor(b:GetFontObject():GetTextColor())")
        .unwrap();
    s.resolve();
    assert_eq!(painted(&s, "overridden").2, Some([0.0, 1.0, 0.0, 1.0]));
}

#[test]
fn propagation_is_scoped_to_the_object_that_changed() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    for (name, face) in [("FontOne", "Fonts\\ONE.TTF"), ("FontTwo", "Fonts\\TWO.TTF")] {
        s.register_font_object(
            name,
            FontObject {
                font: Some(face.into()),
                height: Some(12.0),
                color: Some(crate::color::Rgba8::from_f32([1.0, 1.0, 1.0, 1.0])),
                ..FontObject::default()
            },
        );
    }
    s.run(
        r#"
        f = CreateFrame("Frame", "ScopeHost")
        f:SetWidth(200); f:SetHeight(60); f:SetPoint("CENTER", 0, 0)
        one = f:CreateFontString(nil, "ARTWORK"); one:SetFontObject(FontOne); one:SetText("one")
        two = f:CreateFontString(nil, "ARTWORK"); two:SetFontObject(FontTwo); two:SetText("two")
        FontOne:SetFont("Fonts\\CHANGED.TTF", 30)
    "#,
    )
    .unwrap();
    s.resolve();
    assert_eq!(painted(&s, "one").0, Some("Fonts\\CHANGED.TTF".into()));
    assert_eq!(painted(&s, "two").0, Some("Fonts\\TWO.TTF".into()));
    assert_eq!(painted(&s, "two").1, Some(12.0));
}

// ── CreateFont ──

/// `_Nameplates.lua:149`, `:129` and `:212`, then `!OmniCC/main.lua:40-41`: the name is published
/// even when the return is dropped, and `SetFont`'s return says whether the face loaded.
#[test]
fn create_font_mints_publishes_and_paints() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        Nameplate = { Font = CreateFont("_NameplatesNameplateFont") }
        Nameplate.Font:SetFont("Fonts\\SKURRI.TTF", 10)
        Nameplate.Font:SetTextColor(0, 0.5, 1)

        f = CreateFrame("Frame", "PlateHost")
        f:SetWidth(120); f:SetHeight(20); f:SetPoint("CENTER", 0, 0)
        Name = f:CreateFontString(nil, "ARTWORK")
        Name:SetFontObject(Nameplate.Font)
        Name:SetText("plate")
    "#,
    )
    .expect("_Nameplates' font setup must not raise");
    s.resolve();
    assert_eq!(
        painted(&s, "plate"),
        (
            Some("Fonts\\SKURRI.TTF".into()),
            Some(10.0),
            Some(crate::color::quant::drawn([0.0, 0.5, 1.0, 1.0]))
        )
    );

    assert!(s
        .eval::<bool>("return _NameplatesNameplateFont == Nameplate.Font")
        .unwrap());
    assert_eq!(
        s.eval::<String>("return _NameplatesNameplateFont:GetObjectType()")
            .unwrap(),
        "Font"
    );

    // `!OmniCC/main.lua:40-41`: create, drop the return, then branch on the global's `SetFont`.
    let reverted = s
        .eval::<bool>(
            r#"
            local reverted = false
            if not OmniCCFont then
                CreateFont("OmniCCFont")
                if not OmniCCFont:SetFont("", 20) then
                    reverted = true
                    OmniCCFont:SetFont("Fonts\\FRIZQT__.TTF", 20)
                end
            end
            return reverted
        "#,
        )
        .unwrap();
    assert!(reverted, "SetFont must report an unusable path as false");
    assert_eq!(
        s.eval::<(String, f32, String)>("return OmniCCFont:GetFont()")
            .unwrap(),
        ("Fonts\\FRIZQT__.TTF".to_string(), 20.0, String::new())
    );

    // A name that already names a font object returns that object unchanged (`0x7839ab`).
    s.run("again = CreateFont('_NameplatesNameplateFont')")
        .unwrap();
    assert!(s.eval::<bool>("return again == Nameplate.Font").unwrap());
    assert_eq!(
        s.eval::<(String, f32, String)>("return again:GetFont()")
            .unwrap(),
        ("Fonts\\SKURRI.TTF".to_string(), 10.0, String::new())
    );

    // A nameless `CreateFont` raises; the empty name passes the reference's `lua_isstring` gate.
    assert!(s.run("CreateFont()").is_err());
    s.run("CreateFont('')").unwrap();
}

/// The sequence at `FonzAppraiser/mods/gui/gui.lua:27-30`.
#[test]
fn fonz_appraiser_copies_a_shipped_object_then_overrides_the_face() {
    let read = crate::color::quant::read32;
    let s = script();
    s.register_font_object(
        "GameFontHighlightSmall",
        FontObject {
            font: Some("Fonts\\FRIZQT__.TTF".into()),
            height: Some(10.0),
            color: Some(crate::color::Rgba8::from_f32([1.0, 1.0, 1.0, 1.0])),
            outline: Outline::None,
            monochrome: false,
            justify: Some({
                let mut j = crate::justify::Justify::default();
                j.set_h(JustifyH::Right);
                j
            }),
            shadow: None,
            spacing: None,
        },
    );
    s.run(
        r#"
        small_number_font = CreateFont("FonzAppraiser_NumberFontNormalSmall")
        small_number_font:CopyFontObject(GameFontHighlightSmall)
        small_number_font:SetFont("Fonts\\ARIALN.TTF", 12)
    "#,
    )
    .expect("FonzAppraiser's font setup must not raise");
    assert_eq!(
        s.eval::<(String, f32, String)>("return small_number_font:GetFont()")
            .unwrap(),
        ("Fonts\\ARIALN.TTF".to_string(), 12.0, String::new())
    );
    assert_eq!(
        s.eval::<(f32, f32, f32, f32)>("return small_number_font:GetTextColor()")
            .unwrap(),
        (read(1.0), read(1.0), read(1.0), read(1.0))
    );
    assert_eq!(
        s.eval::<String>("return small_number_font:GetJustifyH()")
            .unwrap(),
        "RIGHT"
    );
    assert_eq!(
        s.eval::<(String, f32, String)>("return GameFontHighlightSmall:GetFont()")
            .unwrap(),
        ("Fonts\\FRIZQT__.TTF".to_string(), 10.0, String::new())
    );
}

/// A registered font object answers the spacing pair (`0x7a09f0`, `0x7a0ab0`), 0 until set, and a
/// set reaches the FontStrings that inherit it.
#[test]
fn a_font_objects_spacing_reaches_its_fontstrings() {
    let s = script();
    s.register_font_object("SomeFont", FontObject::default());
    s.run("FS = CreateFrame('Frame'):CreateFontString() FS:SetFontObject(SomeFont)")
        .unwrap();
    assert_eq!(s.eval::<f32>("return SomeFont:GetSpacing()").unwrap(), 0.0);
    s.run("SomeFont:SetSpacing(4)").unwrap();
    assert_eq!(s.eval::<f32>("return SomeFont:GetSpacing()").unwrap(), 4.0);
    assert_eq!(s.eval::<f32>("return FS:GetSpacing()").unwrap(), 4.0);
}

/// A button's state fonts take the object or the name; they are stored as names and re-resolved at
/// every extract, so a later mutation of the object reaches the label.
#[test]
fn button_state_fonts_take_the_object_and_follow_its_mutation() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.register_font_object(
        "GameFontNormal",
        FontObject {
            font: Some("Fonts\\FRIZQT__.TTF".into()),
            height: Some(12.0),
            color: Some(crate::color::Rgba8::from_f32([1.0, 0.82, 0.0, 1.0])),
            ..FontObject::default()
        },
    );
    s.run(
        r#"
        b = CreateFrame("Button", "StateFontButton")
        b:SetWidth(80); b:SetHeight(22); b:SetPoint("CENTER", 0, 0)
        b:SetText("go")
        b:SetTextFontObject(GameFontNormal)
    "#,
    )
    .expect("the object form must be accepted");
    s.resolve();
    assert_eq!(painted(&s, "go").1, Some(12.0));

    s.run("GameFontNormal:SetFont('Fonts\\\\MORPHEUS.TTF', 22)")
        .unwrap();
    s.resolve();
    assert_eq!(
        painted(&s, "go"),
        (
            Some("Fonts\\MORPHEUS.TTF".into()),
            Some(22.0),
            Some(crate::color::quant::drawn([1.0, 0.82, 0.0, 1.0]))
        )
    );

    s.run("b:SetHighlightFontObject('GameFontNormal')").unwrap();
}

/// The reference gates every merge (`0x770910`/`0x770800`) on the source's has-a-value mask, and a
/// fresh `CreateFont` object's mask is 0, so the FontString keeps what it had.
#[test]
fn an_empty_font_object_copies_nothing_onto_a_fontstring() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.register_font_object(
        "DressedFont",
        FontObject {
            font: Some("Fonts\\FRIZQT__.TTF".into()),
            height: Some(14.0),
            color: Some(crate::color::Rgba8::from_f32([0.2, 0.4, 0.6, 1.0])),
            ..FontObject::default()
        },
    );
    s.run(
        r#"
        f = CreateFrame("Frame", "EmptyFontHost")
        f:SetWidth(120); f:SetHeight(20); f:SetPoint("CENTER", 0, 0)
        fs = f:CreateFontString(nil, "ARTWORK")
        fs:SetFontObject(DressedFont)
        fs:SetText("keep")
        blank = CreateFont("ABlankFont")
        fs:SetFontObject(blank)
    "#,
    )
    .unwrap();
    s.resolve();
    assert_eq!(
        painted(&s, "keep"),
        (
            Some("Fonts\\FRIZQT__.TTF".into()),
            Some(14.0),
            Some([0.2, 0.4, 0.6, 1.0])
        ),
        "an unset property must copy nothing, not blank the region"
    );
    // The link still moves: once the blank font is dressed, the string follows it.
    assert_eq!(
        s.eval::<String>("return fs:GetFontObject():GetName()")
            .unwrap(),
        "ABlankFont"
    );
    s.run("blank:SetFont('Fonts\\\\MORPHEUS.TTF', 22)").unwrap();
    s.resolve();
    assert_eq!(painted(&s, "keep").0, Some("Fonts\\MORPHEUS.TTF".into()));
}

/// The reference's return shape, not a boolean; `!OmniCC/main.lua:41` branches on it.
#[test]
fn set_font_returns_one_or_nil() {
    let s = script();
    s.run("f = CreateFont('ReturnShapeFont')").unwrap();
    assert_eq!(
        s.eval::<f32>("return f:SetFont('Fonts\\\\FRIZQT__.TTF', 12)")
            .unwrap(),
        1.0
    );
    assert!(s.eval::<bool>("return f:SetFont('', 12) == nil").unwrap());
}

/// The justify tokens are one reference table (`.rdata 0x811ad0`) matched whole and case-blind by
/// `SStrCmpI`; a FontString (getters `0x79e5f0`/`0x79e7f0`) and a `<Font>` object answer alike.
#[test]
fn both_tables_speak_one_justify_law() {
    let s = script();
    s.run(
        "seam = CreateFrame('Frame', 'SeamFrame')\n\
         fs = seam:CreateFontString()\n\
         fo = CreateFont('SeamFont')",
    )
    .unwrap();

    for obj in ["fs", "fo"] {
        // The reference's constructor default `0x212`, CENTER | MIDDLE | 0x200, read per axis.
        let h = s
            .eval::<String>(&format!("return {obj}:GetJustifyH()"))
            .unwrap();
        let v = s
            .eval::<String>(&format!("return {obj}:GetJustifyV()"))
            .unwrap();
        assert_eq!((obj, h.as_str()), (obj, "CENTER"));
        assert_eq!((obj, v.as_str()), (obj, "MIDDLE"));

        // Every token round trips, and the match is case-insensitive.
        for (set, get, tokens) in [
            ("SetJustifyH", "GetJustifyH", ["LEFT", "CENTER", "RIGHT"]),
            ("SetJustifyV", "GetJustifyV", ["TOP", "MIDDLE", "BOTTOM"]),
        ] {
            for t in tokens {
                s.run(&format!("{obj}:{set}('{}')", t.to_lowercase()))
                    .unwrap();
                let got = s.eval::<String>(&format!("return {obj}:{get}()")).unwrap();
                assert_eq!(got, t, "{obj}:{set} then {get}");
            }
        }

        // A non-token raises the reference's usage string rather than coercing to CENTER.
        for verb in ["SetJustifyH", "SetJustifyV"] {
            let err = s
                .run(&format!("{obj}:{verb}('MIDDLE_LEFT')"))
                .expect_err("a string outside the six-entry table must raise");
            let err = format!("{err}");
            assert!(
                err.contains("Usage:") && err.contains(verb),
                "{obj}:{verb} raised {err:?}"
            );
        }

        // The match is whole-string: a trailing space is a miss.
        assert!(
            s.run(&format!("{obj}:SetJustifyH('LEFT ')")).is_err(),
            "{obj}:SetJustifyH must not trim its argument"
        );

        // A cross-axis token is in the table and raises nothing: it zeroes the axis on both
        // tables (`0x79fc5d`-`0x79fc75`), which then reads "UNKNOWN".
        s.run(&format!("{obj}:SetJustifyH('TOP')")).unwrap();
        s.run(&format!("{obj}:SetJustifyV('LEFT')")).unwrap();
        assert_eq!(
            s.eval::<String>(&format!(
                "return {obj}:GetJustifyH() .. ' ' .. {obj}:GetJustifyV()"
            ))
            .unwrap(),
            "UNKNOWN UNKNOWN",
            "{obj}"
        );
    }
}

/// `SetJustifyH 0x79fc20` with the other axis's token zeroes the axis, clears the font's own bit
/// and pushes (`0x79fc7c`, `0x79fc84`): a following string reads the cleared axis, and an inheriting
/// font that did it no longer follows its parent's justify.
#[test]
fn a_fonts_cross_axis_justify_clears_the_axis_and_pushes() {
    let s = script();
    s.run(
        "F = CreateFont('CrossAxisF') F:SetFont('Fonts\\FRIZQT__.TTF', 12) F:SetJustifyH('LEFT')
         host = CreateFrame('Frame') fs = host:CreateFontString() fs:SetFontObject(F)
         A = CreateFont('CrossAxisA') B = CreateFont('CrossAxisB') B:SetFontObject(A)
         B:SetJustifyH('TOP') A:SetJustifyH('RIGHT')
         F:SetJustifyH('TOP')",
    )
    .unwrap();
    let got = |e: &str| s.eval::<String>(&format!("return {e}")).unwrap();
    assert_eq!(got("F:GetJustifyH()"), "UNKNOWN");
    assert_eq!(got("fs:GetJustifyH()"), "UNKNOWN", "pushed to the string");
    assert_eq!(got("B:GetJustifyH()"), "UNKNOWN", "B no longer follows A");
}

/// `SetFont` is one routine (`0x79f210`) behind three entries, Font `0x7a0270`, FontString
/// `0x79d4f0` and EditBox `0x797210`; a bad argument raises its usage string (`0x87c69c`).
#[test]
fn set_font_is_one_routine_on_both_tables() {
    let s = script();
    s.run("seam = CreateFrame('Frame', 'SetFontSeamFrame')\nfs = seam:CreateFontString()\nfo = CreateFont('SetFontSeamFont')")
        .unwrap();

    for obj in ["fs", "fo"] {
        let n = s
            .eval::<f32>(&format!(
                "return {obj}:SetFont('Fonts\\\\FRIZQT__.TTF', 12)"
            ))
            .unwrap();
        assert_eq!(n, 1.0, "{obj}:SetFont success");
        assert!(
            s.eval::<bool>(&format!(
                "return {obj}:SetFont('Fonts\\\\FRIZQT__.TTF', 12) == 1"
            ))
            .unwrap(),
            "{obj}:SetFont must answer the number 1, not true"
        );

        // An empty path is a load failure: nil, and nothing raised.
        assert!(
            s.eval::<bool>(&format!("return {obj}:SetFont('', 12) == nil"))
                .unwrap(),
            "{obj}:SetFont('') must answer nil"
        );

        // A missing or non-string path, or a missing height, is an argument error and raises.
        for bad in ["", "'Fonts\\\\FRIZQT__.TTF'", "nil, 12", "{}, 12"] {
            let err = s
                .run(&format!("{obj}:SetFont({bad})"))
                .expect_err("a bad SetFont argument must raise, not answer nil");
            assert!(
                format!("{err}").contains("Usage:") && format!("{err}").contains("SetFont"),
                "{obj}:SetFont({bad}) raised {err}"
            );
        }

        // Both `lua_isstring` and `lua_isnumber` coerce, so either accepts a numeric string.
        assert!(
            s.eval::<bool>(&format!(
                "return {obj}:SetFont('Fonts\\\\FRIZQT__.TTF', '14') == 1"
            ))
            .unwrap(),
            "{obj}:SetFont must accept a numeric string height"
        );
    }
}

/// `SetJustifyH("TOP")` parses (`0x08`), sets nothing in the axis mask `0x07` and raises nothing;
/// `GetJustifyH` then answers `"UNKNOWN"` (`0x6f1a00`, `.data 0x838044`), yet the ui-to-gx
/// translator `0x44d420` pre-sets each axis to 1, so the text still draws centred, not LEFT.
#[test]
fn a_cross_axis_token_erases_the_axis_but_still_draws_centred() {
    let mut s = script();
    s.run(
        "f = CreateFrame('Frame', 'ClearAxisFrame')\n\
         f:SetWidth(200) f:SetHeight(40) f:SetPoint('CENTER', 0, 0)\n\
         fs = f:CreateFontString()\n\
         fs:SetFont('Fonts\\\\FRIZQT__.TTF', 12)\n\
         fs:SetAllPoints(f)\n\
         fs:SetText('erased')",
    )
    .unwrap();

    // A real token first, so the erase below is not a no-op.
    s.run("fs:SetJustifyH('LEFT') fs:SetJustifyV('TOP')")
        .unwrap();
    assert_eq!(s.eval::<String>("return fs:GetJustifyH()").unwrap(), "LEFT");
    assert_eq!(s.eval::<String>("return fs:GetJustifyV()").unwrap(), "TOP");

    // Each setter gets the other axis's token: it clears its own axis and raises nothing.
    s.run("fs:SetJustifyH('TOP')").unwrap();
    s.run("fs:SetJustifyV('CENTER')").unwrap();
    assert_eq!(
        s.eval::<String>("return fs:GetJustifyH()").unwrap(),
        "UNKNOWN",
        "a cleared axis reads UNKNOWN, not the value it used to hold"
    );
    assert_eq!(
        s.eval::<String>("return fs:GetJustifyV()").unwrap(),
        "UNKNOWN"
    );

    s.resolve();
    let (h, v) = s
        .extract()
        .into_iter()
        .find_map(|q| match q.content {
            QuadContent::Text {
                text: Some(ref t),
                justify_h,
                justify_v,
                ..
            } if t == "erased" => Some((justify_h, justify_v)),
            _ => None,
        })
        .expect("the erased string draws");
    assert_eq!(
        (h, v),
        (JustifyH::Center, JustifyV::Middle),
        "a cleared axis draws CENTER/MIDDLE — the gx translator's pre-set 1, NOT LEFT/TOP"
    );
}

/// The shared font block (`0x79f910`, FontString's `0x79dbe0`) is on six tables: FontString, Font,
/// EditBox, MessageFrame, ScrollingMessageFrame and SimpleHTML, not Button. A message frame with no
/// `<FontString>` draws LEFT, and creating its style region (CENTER by default) must keep it LEFT.
#[test]
fn the_font_block_reaches_both_message_frame_tables() {
    let mut s = script();
    s.run(
        "fo = CreateFont('MsgBlockFont')\n\
         fo:SetFont('Fonts\\\\FRIZQT__.TTF', 14)\n\
         mf = CreateFrame('MessageFrame', 'MsgBlockPlain')\n\
         mf:SetWidth(300) mf:SetHeight(80) mf:SetPoint('CENTER', 0, 0)\n\
         smf = CreateFrame('ScrollingMessageFrame', 'MsgBlockScroll')\n\
         smf:SetWidth(300) smf:SetHeight(80) smf:SetPoint('TOPLEFT', 0, 0)",
    )
    .unwrap();

    for obj in ["mf", "smf"] {
        s.run(&format!("{obj}:SetFontObject(fo)")).unwrap();
        assert_eq!(
            s.eval::<String>(&format!("return {obj}:GetFontObject():GetName()"))
                .unwrap(),
            "MsgBlockFont",
            "{obj}:GetFontObject must answer the OBJECT it was given"
        );
        // The shared `SetFont` contract (`0x79f210`).
        assert!(
            s.eval::<bool>(&format!(
                "return {obj}:SetFont('Fonts\\\\MORPHEUS.TTF', 16) == 1"
            ))
            .unwrap(),
            "{obj}:SetFont answers the number 1"
        );
        let (_, height, _) = s
            .eval::<(mlua::Value, f32, String)>(&format!("return {obj}:GetFont()"))
            .unwrap();
        assert_eq!(height, 16.0, "{obj}:GetFont reads back what SetFont wrote");
        // The four-value getters return four values on all six tables (`0x79f9b3`).
        assert_eq!(s.arity(&format!("{obj}:GetShadowColor()")).unwrap(), 4);
    }

    s.run("mf:AddMessage('flush left please')").unwrap();
    s.resolve();
    let j = s
        .extract()
        .into_iter()
        .find_map(|q| match q.content {
            QuadContent::Text {
                text: Some(ref t),
                justify_h,
                ..
            } if t == "flush left please" => Some(justify_h),
            _ => None,
        })
        .expect("the message draws");
    assert_eq!(
        j,
        JustifyH::Left,
        "creating the style region must not re-justify the frame to the FontString default"
    );
}

/// The third argument is looked up as a font object first, and as a template only on a miss
/// (`0x773d39`, then `0x773d47`).
#[test]
fn create_font_string_applies_the_font_object_named_by_its_third_argument() {
    let s = script();
    load(
        &s,
        r#"<Ui>
             <Font name="GameFontNormalSmall" font="Fonts\FRIZQT__.TTF">
               <FontHeight><AbsValue val="12"/></FontHeight>
               <Color r="1" g="0.82" b="0"/>
             </Font>
             <Frame name="Host"/>
           </Ui>"#,
    );

    // The call at `_LazyPig/LazyPigMenu.lua:88`.
    s.run(r#"FS = Host:CreateFontString(nil, "ARTWORK", "GameFontNormalSmall")"#)
        .expect("the font-object form must be accepted");
    let (face, height, _flags) = s
        .eval::<(String, f32, String)>("return FS:GetFont()")
        .expect("the region must carry the font object's face and height");
    assert_eq!(face, "Fonts\\FRIZQT__.TTF");
    assert_eq!(height, 12.0);

    // A name in neither registry raises (`luaL_error`), as an unknown `CreateFrame` template does.
    let err = s
        .run(r#"Bad = Host:CreateFontString(nil, "ARTWORK", "NoSuchFontOrTemplate")"#)
        .expect_err("a name in neither registry must raise");
    assert!(
        err.to_string().contains("NoSuchFontOrTemplate"),
        "the raise must name what was looked up: {err}"
    );

    // `CreateTexture` takes the same third argument through the same resolver.
    s.run(r#"TX = Host:CreateTexture(nil, "OVERLAY")"#)
        .expect("the two-argument form still works");
}

/// The reference answers 1 or nil (`0x79f345`/`0x79f361`), nil when the font factory (`0x5c1ae0`)
/// finds no readable file; with no host probe, any non-empty path answers 1.
#[test]
fn set_font_answers_the_hosts_load_verdict_when_there_is_a_host() {
    let mut s = script();
    s.run("FS = CreateFrame('Frame'):CreateFontString()")
        .unwrap();

    // No probe: any non-empty path is 1, an empty one is nil.
    assert!(s
        .eval::<bool>(r"return FS:SetFont('Interface\\Addons\\Nope\\Fonts\\x.ttf', 12) == 1")
        .unwrap());
    assert!(s.eval::<bool>("return FS:SetFont('', 12) == nil").unwrap());

    // With a probe the store decides, and a refused face is not adopted.
    s.set_font_probe(Box::new(|path| {
        path.eq_ignore_ascii_case("interface\\addons\\msbt\\fonts\\porky.ttf")
            || path.eq_ignore_ascii_case("fonts\\frizqt__.ttf")
    }));
    assert!(s
        .eval::<bool>(r"return FS:SetFont('Fonts\\FRIZQT__.TTF', 12) == 1")
        .unwrap());
    assert!(
        s.eval::<bool>(r"return FS:SetFont('Interface\\AddOns\\MSBT\\Fonts\\porky.ttf', 18) == 1")
            .unwrap(),
        "an addon's own face, spelled in its own case, must load"
    );
    assert!(
        s.eval::<bool>(r"return FS:SetFont('Interface\\AddOns\\MSBT\\Fonts\\gone.ttf', 30) == nil")
            .unwrap(),
        "a path the store does not hold is the reference's falsey load failure"
    );
    let (face, height, _) = s
        .eval::<(String, f32, String)>("return FS:GetFont()")
        .unwrap();
    assert_eq!(
        face, "Interface\\AddOns\\MSBT\\Fonts\\porky.ttf",
        "the refused face must not replace the one that loaded"
    );
    assert_eq!(
        height, 18.0,
        "a refused load writes nothing, the height included (`0x771571`)"
    );
}

/// MSBT's per-event sequence over the stock `MasterFont` (`Fonts.xml:55`, a `<Shadow>` only): its
/// own face, size, outline, colour and fade reach the quad, and the shadow is inherited.
#[test]
fn msbt_paints_its_own_face_size_outline_and_fade_over_the_font_object_it_inherits() {
    let mut s = script();
    s.set_screen_size(1600.0, 900.0);
    load(
        &s,
        r#"<Ui>
             <Font name="MasterFont" virtual="true">
               <Shadow><Offset><AbsDimension x="1" y="-1"/></Offset><Color r="0" g="0" b="0"/></Shadow>
             </Font>
             <Frame name="MSBTFrameIncoming" parent="UIParent">
               <Size><AbsDimension x="24" y="24"/></Size>
               <Anchors><Anchor point="BOTTOM" relativePoint="CENTER" relativeTo="UIParent"/></Anchors>
               <Layers><Layer level="ARTWORK">
                 <FontString name="$parentText1" inherits="MasterFont">
                   <Anchors><Anchor point="BOTTOMRIGHT"/></Anchors>
                 </FontString>
               </Layer></Layers>
             </Frame>
           </Ui>"#,
    );
    s.run(
        r#"
        local fs = getglobal("MSBTFrameIncomingText1")
        fs:ClearAllPoints()
        fs:SetFont("Interface\\Addons\\MikScrollingBattleText\\Fonts\\porky.ttf", 18, "OUTLINE")
        fs:SetTextColor(1, 1, 1)
        fs:SetText("-64")
        fs:SetAlpha(0.5)
        fs:SetPoint("BOTTOMRIGHT", 0, 0)
    "#,
    )
    .unwrap();
    s.resolve();
    let q = s
        .extract()
        .into_iter()
        .find(|q| matches!(&q.content, QuadContent::Text { text: Some(t), .. } if t == "-64"))
        .expect("MSBT's text quad");
    assert_eq!(
        q.alpha,
        127.0 / 255.0,
        "the scroll animation's fade is the quad's alpha, the byte `SetAlpha(0.5)` stores"
    );
    match q.content {
        QuadContent::Text {
            ref font,
            font_height,
            outline,
            color,
            shadow,
            ..
        } => {
            assert_eq!(
                font.as_deref(),
                Some("Interface\\Addons\\MikScrollingBattleText\\Fonts\\porky.ttf"),
                "the addon's own face, not the fallback"
            );
            assert_eq!(font_height, Some(18.0), "the profile's master size");
            assert_eq!(outline, Outline::Normal, "its OUTLINE flag");
            assert_eq!(color, Some([1.0, 1.0, 1.0, 1.0]));
            assert!(
                shadow.is_some(),
                "MasterFont's <Shadow> is the one axis the string never set, so it inherits"
            );
        }
        ref other => panic!("expected a Text quad, got {other:?}"),
    }
    assert_eq!(
        s.eval::<(String, f32, String)>("return MSBTFrameIncomingText1:GetFont()")
            .unwrap(),
        (
            "Interface\\Addons\\MikScrollingBattleText\\Fonts\\porky.ttf".to_string(),
            18.0,
            "OUTLINE".to_string()
        )
    );
}

/// The flags word of the text quad reading `text`: `(outline, monochrome)`.
fn flags_of(s: &UiScript, text: &str) -> (Outline, bool) {
    s.extract()
        .into_iter()
        .find_map(|q| match q.content {
            QuadContent::Text {
                text: Some(ref t),
                outline,
                monochrome,
                ..
            } if t == text => Some((outline, monochrome)),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no text quad reading {text:?}"))
}

/// The action button's `HotKey` (`ActionButtonTemplate.xml`) inherits `NumberFontNormalSmallGray`,
/// which inherits `NumberFontNormalSmall`'s `monochrome="true"` (`Fonts.xml`): the hotkey draws
/// monochrome and THICK.
#[test]
fn the_hotkey_font_draws_monochrome_through_its_inherited_object() {
    let mut s = script();
    load(
        &s,
        r#"<Ui>
             <Font name="NumberFontNormalSmall" font="Fonts\ARIALN.TTF" outline="THICK" virtual="true" monochrome="true">
               <FontHeight><AbsValue val="12"/></FontHeight>
             </Font>
             <Font name="NumberFontNormalSmallGray" inherits="NumberFontNormalSmall" outline="THICK" virtual="true">
               <Color r="0.6" g="0.6" b="0.6"/>
             </Font>
             <Font name="NumberFontNormal" font="Fonts\ARIALN.TTF" outline="NORMAL" virtual="true">
               <FontHeight><AbsValue val="14"/></FontHeight>
             </Font>
             <Frame name="Bar" parent="UIParent">
               <Size><AbsDimension x="36" y="36"/></Size>
               <Anchors><Anchor point="CENTER"/></Anchors>
               <Layers><Layer level="ARTWORK">
                 <FontString name="$parentHotKey" inherits="NumberFontNormalSmallGray" text="2">
                   <Anchors><Anchor point="TOPRIGHT"/></Anchors>
                 </FontString>
                 <FontString name="$parentCount" inherits="NumberFontNormal" text="4">
                   <Anchors><Anchor point="BOTTOMRIGHT"/></Anchors>
                 </FontString>
               </Layer></Layers>
             </Frame>
           </Ui>"#,
    );
    s.resolve();
    assert_eq!(flags_of(&s, "2"), (Outline::Thick, true));
    assert_eq!(flags_of(&s, "4"), (Outline::Normal, false));
}

/// `SetFont`'s flags are found as substrings (`0x6f1a90`), `MONOCHROME` among them, and replace the
/// whole flags word, the inherited monochrome bit included.
#[test]
fn set_font_reads_and_clears_monochrome() {
    let mut s = script();
    load(
        &s,
        r#"<Ui>
             <Font name="Mono" font="Fonts\ARIALN.TTF" monochrome="true">
               <FontHeight><AbsValue val="12"/></FontHeight>
             </Font>
             <Frame name="F" parent="UIParent">
               <Size><AbsDimension x="36" y="36"/></Size>
               <Anchors><Anchor point="CENTER"/></Anchors>
               <Layers><Layer level="ARTWORK">
                 <FontString name="$parentA" inherits="Mono" text="a">
                   <Anchors><Anchor point="TOP"/></Anchors>
                 </FontString>
                 <FontString name="$parentB" inherits="Mono" text="b">
                   <Anchors><Anchor point="BOTTOM"/></Anchors>
                 </FontString>
               </Layer></Layers>
             </Frame>
           </Ui>"#,
    );
    s.run(
        r#"
        FA:SetFont("Fonts\\ARIALN.TTF", 12, "OUTLINE")
        FB:SetFont("Fonts\\ARIALN.TTF", 12, "outline, monochrome")
    "#,
    )
    .unwrap();
    s.resolve();
    assert_eq!(flags_of(&s, "a"), (Outline::Normal, false));
    assert_eq!(flags_of(&s, "b"), (Outline::Normal, true));
}

/// `CSimpleFont`'s constructor zeroes the shadow colour (`0x783a72`): a `CreateFont` font, an
/// EditBox's and a message frame's read transparent black, and `SetShadowOffset` keeps it, so a
/// following string takes an invisible shadow. Only a FontString's own default is opaque
/// (`0x770da1`).
#[test]
fn a_fonts_unset_shadow_is_transparent() {
    let s = script();
    s.run(
        "F = CreateFont('UnsetShadowF')
         host = CreateFrame('Frame') fs = host:CreateFontString() plain = host:CreateFontString()
         eb = CreateFrame('EditBox') mf = CreateFrame('MessageFrame')
         smf = CreateFrame('ScrollingMessageFrame') html = CreateFrame('SimpleHTML')",
    )
    .unwrap();
    let alpha = |obj: &str| {
        s.eval::<Vec<f32>>(&format!("return {{ {obj}:GetShadowColor() }}"))
            .unwrap()
    };
    for obj in ["F", "eb", "mf", "smf", "html"] {
        assert_eq!(alpha(obj), vec![0.0, 0.0, 0.0, 0.0], "{obj}");
    }
    // SimpleHTML's four element fonts come from the same constructor (`0x789e7c`), and
    // `SetShadowOffset` keeps their colour.
    s.run("html:SetShadowOffset('H1', 1, -1)").unwrap();
    for elem in ["P", "H1", "H2", "H3"] {
        let c = s
            .eval::<Vec<f32>>(&format!("return {{ html:GetShadowColor('{elem}') }}"))
            .unwrap();
        assert_eq!(c, vec![0.0, 0.0, 0.0, 0.0], "SimpleHTML {elem}");
    }
    assert_eq!(
        alpha("plain")[3],
        crate::color::quant::read32(1.0),
        "a FontString's own shadow"
    );
    s.run("F:SetShadowOffset(1, -1) fs:SetFontObject(F) eb:SetShadowOffset(1, -1)")
        .unwrap();
    assert_eq!(alpha("F"), vec![0.0, 0.0, 0.0, 0.0]);
    assert_eq!(
        alpha("fs"),
        vec![0.0, 0.0, 0.0, 0.0],
        "the follower's shadow"
    );
    assert_eq!(alpha("eb"), vec![0.0, 0.0, 0.0, 0.0]);
}
