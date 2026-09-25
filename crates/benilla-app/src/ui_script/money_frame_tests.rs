//! The stock `MoneyFrame` kit, driven as an addon drives it: a frame on each template, a type set
//! and switched, the coins read back.

use benilla_ui::script::{QuadContent, UiScript};

use super::test_ui::load_ui as load_xml;

/// The kit plus the two frames an addon would declare: one on each template.
fn harness(money: u64) -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.set_money(money);
    // A flat 8 px per digit, so every expected width reads as `digits * 8 + icon`.
    s.set_text_measurer(Box::new(super::FixedWidthFont(8.0)));
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml"); // `message`, the kit's own error path
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");

    let doc = benilla_ui::framexml::parse(
        r#"<Ui>
            <Frame name="TestPurse" inherits="SmallMoneyFrameTemplate">
                <Anchors><Anchor point="TOPLEFT"/></Anchors>
            </Frame>
            <Frame name="TestBigPurse" inherits="MoneyFrameTemplate">
                <Anchors><Anchor point="TOPLEFT"><Offset><AbsDimension x="0" y="-40"/></Offset></Anchor></Anchors>
            </Frame>
        </Ui>"#,
    )
    .unwrap();
    let report = benilla_ui::loader::load(&s, &doc, &|_| None);
    assert!(
        report.errors.is_empty(),
        "loader errors: {:?}",
        report.errors
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s
}

/// The three coins' text: gold, silver, copper.
fn coins(s: &UiScript, frame: &str) -> (String, String, String) {
    let one = |d: &str| {
        s.eval::<String>(&format!("return {frame}{d}ButtonText:GetText() or ''"))
            .unwrap()
    };
    (one("Gold"), one("Silver"), one("Copper"))
}

/// Which coins show: gold, silver, copper.
fn shown(s: &UiScript, frame: &str) -> (bool, bool, bool) {
    let one = |d: &str| {
        s.eval::<bool>(&format!("return {frame}{d}Button:IsShown()"))
            .unwrap()
    };
    (one("Gold"), one("Silver"), one("Copper"))
}

/// Both `MoneyFrame_OnLoad` and `SmallMoneyFrame_OnLoad` end in `MoneyFrame_SetType("PLAYER")`.
#[test]
fn a_money_frame_loads_as_the_player_purse_and_splits_the_denominations() {
    benilla_formats::wow_data_or_skip!();
    let s = harness(12_345); // 1g 23s 45c
    assert_eq!(
        s.eval::<String>("return TestPurse.moneyType").unwrap(),
        "PLAYER"
    );
    assert_eq!(
        coins(&s, "TestPurse"),
        ("1".into(), "23".into(), "45".into())
    );
    assert_eq!(
        coins(&s, "TestBigPurse"),
        ("1".into(), "23".into(), "45".into())
    );
    assert_eq!(
        s.eval::<i64>("return TestPurse.staticMoney").unwrap(),
        12_345,
        "the frame remembers what it is displaying (ref l.212)"
    );
    // `small`, the two OnLoads' only difference, picks the 13 px coin over the 19 px one.
    assert_eq!(s.eval::<i64>("return TestPurse.small").unwrap(), 1);
    assert!(s.eval::<bool>("return TestBigPurse.small == nil").unwrap());
}

#[test]
fn set_type_rewires_the_source_the_mouse_and_the_collapse() {
    benilla_formats::wow_data_or_skip!();
    let s = harness(12_345);

    for d in ["Gold", "Silver", "Copper"] {
        assert!(
            s.eval::<bool>(&format!("return TestPurse{d}Button:IsMouseEnabled()"))
                .unwrap(),
            "PLAYER has canPickup = 1, so {d} takes the mouse"
        );
    }

    s.run("TestPurse.staticMoney = 7 this = TestPurse MoneyFrame_SetType(\"STATIC\") this = nil")
        .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.eval::<String>("return TestPurse.moneyType").unwrap(),
        "STATIC"
    );
    assert_eq!(coins(&s, "TestPurse"), ("0".into(), "0".into(), "7".into()));
    for d in ["Gold", "Silver", "Copper"] {
        assert!(
            !s.eval::<bool>(&format!("return TestPurse{d}Button:IsMouseEnabled()"))
                .unwrap(),
            "STATIC has no canPickup, so {d} does not take the mouse"
        );
    }
    // STATIC collapses without `showSmallerCoins`: 7 copper is copper alone.
    assert_eq!(shown(&s, "TestPurse"), (false, false, true));

    s.run("this = TestPurse MoneyFrame_SetType(\"PLAYER\") this = nil")
        .unwrap();
    assert_eq!(
        coins(&s, "TestPurse"),
        ("1".into(), "23".into(), "45".into())
    );
    assert!(s
        .eval::<bool>("return TestPurseGoldButton:IsMouseEnabled()")
        .unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

#[test]
fn showsmallercoins_is_what_keeps_the_zero_coins_visible() {
    benilla_formats::wow_data_or_skip!();
    let s = harness(50_000); // exactly 5g

    assert_eq!(coins(&s, "TestPurse"), ("5".into(), "0".into(), "0".into()));
    assert_eq!(
        shown(&s, "TestPurse"),
        (true, true, true),
        "PLAYER showSmallerCoins = \"Backpack\" keeps the zero silver and copper (ref l.30)"
    );

    s.run(
        "TestPurse.staticMoney = 50000 this = TestPurse MoneyFrame_SetType(\"STATIC\") this = nil",
    )
    .unwrap();
    assert_eq!(
        shown(&s, "TestPurse"),
        (true, false, false),
        "STATIC collapses with no showSmallerCoins — 5g is gold alone (ref l.32-38)"
    );

    // Under a collapsing type, a copper-only amount drops the leading zero coins.
    s.run("TestPurse.staticMoney = 42 this = TestPurse MoneyFrame_UpdateMoney() this = nil")
        .unwrap();
    assert_eq!(shown(&s, "TestPurse"), (false, false, true));
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

#[test]
fn update_paints_a_static_frame_by_name_and_resizes_it() {
    benilla_formats::wow_data_or_skip!();
    let s = harness(0);
    s.run("this = TestPurse MoneyFrame_SetType(\"STATIC\") this = nil")
        .unwrap();

    s.run("MoneyFrame_Update(\"TestPurse\", 20304)").unwrap(); // 2g 3s 4c
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(coins(&s, "TestPurse"), ("2".into(), "3".into(), "4".into()));
    assert_eq!(shown(&s, "TestPurse"), (true, true, true));
    assert_eq!(
        s.eval::<i64>("return TestPurse.staticMoney").unwrap(),
        20_304
    );

    // A coin is one 8 px digit plus the 13 px small icon, 21; the frame packs three with the -4
    // spacing: 13 + 21 * 3 + 4 * 2 = 84. The text measure answers inside the Lua call, so this
    // holds on the first pass.
    assert_eq!(
        s.eval::<f64>("return TestPurseGoldButton:GetWidth()")
            .unwrap(),
        21.0
    );
    assert_eq!(s.eval::<f64>("return TestPurse:GetWidth()").unwrap(), 84.0);

    // Collapse away two denominations and the frame shrinks to match: 13 + 21 = 34.
    s.run("MoneyFrame_Update(\"TestPurse\", 9)").unwrap();
    assert_eq!(shown(&s, "TestPurse"), (false, false, true));
    assert_eq!(s.eval::<f64>("return TestPurse:GetWidth()").unwrap(), 34.0);
}

/// `this.small` unset picks `MONEY_ICON_WIDTH` (19) and `MONEY_BUTTON_SPACING` (-4).
#[test]
fn the_large_template_measures_with_the_nineteen_pixel_icon() {
    benilla_formats::wow_data_or_skip!();
    let s = harness(20_304);
    assert_eq!(
        s.eval::<f64>("return TestBigPurseGoldButton:GetWidth()")
            .unwrap(),
        8.0 + 19.0
    );
    assert_eq!(
        s.eval::<f64>("return TestBigPurse:GetWidth()").unwrap(),
        19.0 + 27.0 * 3.0 + 8.0,
        "19 base + three 27px coins, packed with two -4px gaps"
    );
    assert_eq!(
        s.eval::<i64>("return MONEY_ICON_WIDTH").unwrap(),
        19,
        "the reference's own constants ship with the kit — addons read them"
    );
    assert_eq!(s.eval::<i64>("return COPPER_PER_GOLD").unwrap(), 10_000);
}

/// `MoneyFrame_SetType` returns on an unknown type before touching anything (`MoneyFrame.lua:144`).
#[test]
fn an_unknown_money_type_leaves_the_frame_on_the_one_it_had() {
    benilla_formats::wow_data_or_skip!();
    let s = harness(12_345);
    s.run("this = TestPurse MoneyFrame_SetType(\"NOT_A_TYPE\") this = nil")
        .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.eval::<String>("return TestPurse.moneyType").unwrap(),
        "PLAYER",
        "the bad SetType returned before touching info/moneyType"
    );
    assert_eq!(
        coins(&s, "TestPurse"),
        ("1".into(), "23".into(), "45".into())
    );
}

/// `SetMoneyFrameColor` sets the three buttons' text colour only (`MoneyFrame.lua:348`).
#[test]
fn set_money_frame_color_recolours_the_digits_and_not_the_icons() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness(12_345); // 1g 23s 45c: three distinct digit strings
    s.run("SetMoneyFrameColor(\"TestPurse\", 1.0, 0.1, 0.1)")
        .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s.resolve();
    let quads = s.extract();

    // Both frames show the purse, so six digit quads; only the named frame's three turn red.
    let digit_colors: Vec<[f32; 4]> = quads
        .iter()
        .filter_map(|q| match &q.content {
            QuadContent::Text {
                text: Some(t),
                color: Some(c),
                ..
            } if t == "1" || t == "23" || t == "45" => Some(*c),
            _ => None,
        })
        .collect();
    assert_eq!(digit_colors.len(), 6, "two frames * three denominations");
    let reddened = |c: &[f32; 4]| {
        (c[0] - 1.0).abs() < 0.01 && (c[1] - 0.1).abs() < 0.01 && (c[2] - 0.1).abs() < 0.01
    };
    assert_eq!(
        digit_colors.iter().filter(|c| reddened(c)).count(),
        3,
        "only TestPurse's digits reddened, got {digit_colors:?}"
    );

    let icon_colors: Vec<[f32; 4]> = quads
        .iter()
        .filter_map(|q| match &q.content {
            QuadContent::Texture {
                path: Some(p),
                color,
                ..
            } if p.contains("UI-MoneyIcons") => Some(color.unwrap_or([1.0; 4])),
            _ => None,
        })
        .collect();
    assert_eq!(icon_colors.len(), 6, "six coin icons drew");
    for c in &icon_colors {
        assert!(
            c[1] > 0.9 && c[2] > 0.9,
            "the coin art is untinted, got {c:?}"
        );
    }
}

/// Addons read `MoneyTypeInfo` directly; the send-mail rows read their engine getters.
#[test]
fn the_money_type_table_carries_all_seven_reference_types() {
    benilla_formats::wow_data_or_skip!();
    let s = harness(12_345);
    for t in [
        "PLAYER",
        "STATIC",
        "AUCTION",
        "PLAYER_TRADE",
        "TARGET_TRADE",
        "SEND_MAIL",
        "SEND_MAIL_COD",
    ] {
        assert!(
            s.eval::<bool>(&format!(
                "return MoneyTypeInfo[\"{t}\"] ~= nil and type(MoneyTypeInfo[\"{t}\"].UpdateFunc) == 'function'"
            ))
            .unwrap(),
            "MoneyTypeInfo[\"{t}\"] with an UpdateFunc"
        );
    }
    for t in ["SEND_MAIL", "SEND_MAIL_COD"] {
        s.run(&format!(
            "this = TestPurse MoneyFrame_SetType(\"{t}\") this = nil"
        ))
        .unwrap();
        assert!(
            s.errors().is_empty(),
            "{t}: script errors: {:?}",
            s.errors()
        );
        assert_eq!(
            s.eval::<i64>("return TestPurse.staticMoney").unwrap(),
            0,
            "{t} reads its engine getter, which is 0 with no send-mail amount set"
        );
    }
}

/// `MoneyInputFrame.lua` is the only stock caller of `EditBox:SetNumber`.
#[test]
fn the_chains_money_input_frame_splits_an_amount_across_its_three_boxes() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = UiScript::new().unwrap();
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua"); // COPPER_PER_GOLD and COPPER_PER_SILVER
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyInputFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyInputFrame.xml");

    let doc = benilla_ui::framexml::parse(
        r#"<Ui>
            <Frame name="TestAmount" inherits="MoneyInputFrameTemplate">
                <Anchors><Anchor point="TOPLEFT"/></Anchors>
            </Frame>
        </Ui>"#,
    )
    .unwrap();
    let report = benilla_ui::loader::load_in(&s, &doc, "test", &|_: &str| None);
    assert!(report.errors.is_empty(), "{:?}", report.errors);

    // 1234g 56s 78c.
    s.run("MoneyInputFrame_SetCopper(TestAmount, 12345678)")
        .unwrap();
    for (box_name, want) in [("Gold", "1234"), ("Silver", "56"), ("Copper", "78")] {
        assert_eq!(
            s.eval::<String>(&format!("return TestAmount{box_name}:GetText()"))
                .unwrap(),
            want,
            "TestAmount{box_name}"
        );
    }
    assert_eq!(
        s.eval::<f64>("return MoneyInputFrame_GetCopper(TestAmount)")
            .unwrap(),
        12345678.0,
        "the amount round-trips back out of the three boxes"
    );

    // An empty box's `GetNumber()` is already 0, so `SetNumber` is skipped and the boxes stay
    // empty (`MoneyInputFrame.lua:46`).
    s.run("MoneyInputFrame_ResetMoney(TestAmount)").unwrap();
    s.run("MoneyInputFrame_SetCopper(TestAmount, 0)").unwrap();
    for box_name in ["Gold", "Silver", "Copper"] {
        assert_eq!(
            s.eval::<String>(&format!("return TestAmount{box_name}:GetText()"))
                .unwrap(),
            "",
            "TestAmount{box_name} stays empty at zero"
        );
    }
    assert_eq!(
        s.eval::<f64>("return MoneyInputFrame_GetCopper(TestAmount)")
            .unwrap(),
        0.0
    );
}
