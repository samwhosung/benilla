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
    assert_eq!((r, g, b), (1.0, 0.82, 0.0));
    assert_eq!(
        s.eval::<(f32, f32)>("return GameFontNormal:GetShadowOffset()")
            .unwrap(),
        (1.0, -1.0)
    );
    assert_eq!(
        s.eval::<(f32, f32, f32, f32)>("return GameFontNormal:GetShadowColor()")
            .unwrap(),
        (0.0, 0.0, 0.0, 1.0)
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
             <Font name="GameTooltipHeaderText" inherits="GameTooltipText">
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

/// `inherits=` is flattened once, at load, where the reference links it live (`0x770c60`), and
/// the published object carries the flattened values.
#[test]
fn an_inheriting_font_still_flattens_to_the_same_values() {
    let s = script();
    load(
        &s,
        r#"<Ui>
             <Font name="MasterFont" font="Fonts\FRIZQT__.TTF" justifyH="CENTER">
               <FontHeight><AbsValue val="10"/></FontHeight>
               <Color r="1" g="1" b="1"/>
               <Shadow><Offset><AbsDimension x="1" y="-1"/></Offset><Color r="0" g="0" b="0"/></Shadow>
             </Font>
             <Font name="DerivedFont" inherits="MasterFont" outline="NORMAL">
               <FontHeight><AbsValue val="18"/></FontHeight>
             </Font>
           </Ui>"#,
    );
    let derived = s.font_object("DerivedFont").expect("registered");
    assert_eq!(derived.font.as_deref(), Some("Fonts\\FRIZQT__.TTF"));
    assert_eq!(derived.height, Some(18.0));
    assert_eq!(derived.color, Some([1.0, 1.0, 1.0, 1.0]));
    assert_eq!(derived.outline, Outline::Normal);
    assert_eq!(derived.justify_h, Some(JustifyH::Center));
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
            color: Some([0.25, 0.5, 0.75, 1.0]),
            outline: Outline::Thick,
            justify_h: None,
            justify_v: None,
            shadow: None,
        },
    );
    s.run(
        r#"
        f = CreateFrame("Frame", "TwoWays")
        f:SetWidth(200); f:SetHeight(40); f:SetPoint("CENTER", 0, 0)
        byObject = f:CreateFontString(nil, "ARTWORK")
        byObject:SetText("obj")
        byObject:SetFontObject(GameFontHighlightSmall)
        byName = f:CreateFontString(nil, "ARTWORK")
        byName:SetText("str")
        byName:SetFontObject("GameFontHighlightSmall")
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
            Some([0.25, 0.5, 0.75, 1.0])
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
            color: Some([1.0, 1.0, 1.0, 1.0]),
            outline: Outline::None,
            justify_h: None,
            justify_v: None,
            shadow: None,
        },
    );
    s.run(
        r#"
        f = CreateFrame("Frame", "DdRow")
        f:SetWidth(120); f:SetHeight(16); f:SetPoint("CENTER", 0, 0)
        button = { text = f:CreateFontString(nil, "ARTWORK") }
        button.text:SetText("row")
        button.text:SetFontObject(GameFontHighlightSmall)
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
            color: Some([1.0, 1.0, 1.0, 1.0]),
            outline: Outline::None,
            justify_h: None,
            justify_v: None,
            shadow: None,
        },
    );
    s.run(
        r#"
        f = CreateFrame("Frame", "ThemeHost")
        f:SetWidth(200); f:SetHeight(60); f:SetPoint("CENTER", 0, 0)
        a = f:CreateFontString(nil, "ARTWORK"); a:SetText("inherited")
        a:SetFontObject(ThemeFont)
        b = f:CreateFontString(nil, "ARTWORK"); b:SetText("overridden")
        b:SetFontObject(ThemeFont)
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
                color: Some([1.0, 1.0, 1.0, 1.0]),
                ..FontObject::default()
            },
        );
    }
    s.run(
        r#"
        f = CreateFrame("Frame", "ScopeHost")
        f:SetWidth(200); f:SetHeight(60); f:SetPoint("CENTER", 0, 0)
        one = f:CreateFontString(nil, "ARTWORK"); one:SetText("one"); one:SetFontObject(FontOne)
        two = f:CreateFontString(nil, "ARTWORK"); two:SetText("two"); two:SetFontObject(FontTwo)
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
        Name:SetText("plate")
        Name:SetFontObject(Nameplate.Font)
    "#,
    )
    .expect("_Nameplates' font setup must not raise");
    s.resolve();
    assert_eq!(
        painted(&s, "plate"),
        (
            Some("Fonts\\SKURRI.TTF".into()),
            Some(10.0),
            Some([0.0, 0.5, 1.0, 1.0])
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
    let s = script();
    s.register_font_object(
        "GameFontHighlightSmall",
        FontObject {
            font: Some("Fonts\\FRIZQT__.TTF".into()),
            height: Some(10.0),
            color: Some([1.0, 1.0, 1.0, 1.0]),
            outline: Outline::None,
            justify_h: Some(JustifyH::Right),
            justify_v: None,
            shadow: None,
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
        (1.0, 1.0, 1.0, 1.0)
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

/// Line spacing is not built: the reference's `SetSpacing` is absent here, so a call raises rather
/// than store a number nothing draws.
#[test]
fn the_unmodelled_spacing_pair_fails_loudly() {
    let s = script();
    s.register_font_object("SomeFont", FontObject::default());
    assert!(s.run("SomeFont:SetSpacing(4)").is_err());
    assert!(s.eval::<bool>("return SomeFont.SetSpacing == nil").unwrap());
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
            color: Some([1.0, 0.82, 0.0, 1.0]),
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
            Some([1.0, 0.82, 0.0, 1.0])
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
            color: Some([0.2, 0.4, 0.6, 1.0]),
            ..FontObject::default()
        },
    );
    s.run(
        r#"
        f = CreateFrame("Frame", "EmptyFontHost")
        f:SetWidth(120); f:SetHeight(20); f:SetPoint("CENTER", 0, 0)
        fs = f:CreateFontString(nil, "ARTWORK")
        fs:SetText("keep")
        fs:SetFontObject(DressedFont)
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

        // A cross-axis token is in the table and raises nothing. The reference then clears the
        // axis; the FontString does too, the `<Font>` object leaves it as it was.
        s.run(&format!("{obj}:SetJustifyH('TOP')")).unwrap();
        s.run(&format!("{obj}:SetJustifyV('LEFT')")).unwrap();
    }
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
        s.eval::<bool>(r"return FS:SetFont('Interface\\AddOns\\MSBT\\Fonts\\gone.ttf', 18) == nil")
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
    assert_eq!(height, 18.0, "…while the height, which never fails, is set");
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
        q.alpha, 0.5,
        "the scroll animation's fade is the quad's alpha"
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
