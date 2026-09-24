//! The stock `Interface\FrameXML\MoneyFrame.xml` — the reference's `MoneyFrameTemplate` /
//! `SmallMoneyFrameTemplate` kit and the `MoneyTypeInfo` table behind it (a name the
//! shipped 1.12 UI defines is a name we publish too — under our own implementation of it, 1260).
//!
//! Eight shipped windows consume this file (1937 converged the coin cluster onto it; 1962 put the
//! file itself on the chain). These tests exercise it the way an addon does — declare a frame on
//! the template, point it at a type, change the type, read the coins back — because that is who
//! calls it.
//!
//! What they guard: the type switch actually rewiring where the number comes from and whether the
//! coins take the mouse; the collapse rule per type (`collapse`, `showSmallerCoins`, `fixedWidth`);
//! the denomination split; `MoneyFrame_Update`'s by-name entry point over a STATIC frame; and
//! `SetMoneyFrameColor` recolouring the digits and not the icons.

use benilla_ui::script::{QuadContent, UiScript};

use super::test_ui::load_ui as load_xml;

/// The kit plus the two frames an addon would declare: one on each template.
fn harness(money: u64) -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.set_money(money);
    // The digit advances the app feeds once per atlas scale. Fed here so the width arithmetic
    // below is deliberate rather than an artefact of an unfed VM — a flat 8px per digit makes
    // every expected number readable as `digits x 8 + icon`.
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

/// The three coins' text, gold → silver → copper.
fn coins(s: &UiScript, frame: &str) -> (String, String, String) {
    let one = |d: &str| {
        s.eval::<String>(&format!("return {frame}{d}ButtonText:GetText() or ''"))
            .unwrap()
    };
    (one("Gold"), one("Silver"), one("Copper"))
}

/// Which of the three coins are showing, gold → silver → copper.
fn shown(s: &UiScript, frame: &str) -> (bool, bool, bool) {
    let one = |d: &str| {
        s.eval::<bool>(&format!("return {frame}{d}Button:IsShown()"))
            .unwrap()
    };
    (one("Gold"), one("Silver"), one("Copper"))
}

/// A frame on either template loads as type PLAYER showing the purse, split into denominations —
/// the reference's `MoneyFrame_OnLoad` / `SmallMoneyFrame_OnLoad`, both of which end in
/// `MoneyFrame_SetType("PLAYER")`.
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
    // `small` is what picks the 13px coin over the 19px one — the only difference between the two
    // OnLoads, and the reason both templates exist.
    assert_eq!(s.eval::<i64>("return TestPurse.small").unwrap(), 1);
    assert!(s.eval::<bool>("return TestBigPurse.small == nil").unwrap());
}

/// **`MoneyFrame_SetType` switching a frame between types** — the verb 11 corpus addons call, and
/// the one that has to rewire three things at once: where the number comes from, whether the coins
/// take the mouse (`canPickup`), and how they collapse.
#[test]
fn set_type_rewires_the_source_the_mouse_and_the_collapse() {
    benilla_formats::wow_data_or_skip!();
    let s = harness(12_345);

    // PLAYER: canPickup, so all three coins are mouse-enabled.
    for d in ["Gold", "Silver", "Copper"] {
        assert!(
            s.eval::<bool>(&format!("return TestPurse{d}Button:IsMouseEnabled()"))
                .unwrap(),
            "PLAYER has canPickup = 1, so {d} takes the mouse"
        );
    }

    // → STATIC: the number now comes from the frame's own staticMoney, and the coins go inert.
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
    // STATIC collapses and does NOT showSmallerCoins: 7 copper is copper alone.
    assert_eq!(shown(&s, "TestPurse"), (false, false, true));

    // → back to PLAYER: the purse returns, and so does the mouse.
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

/// The **collapse rule**, which is the whole reason `MoneyTypeInfo` has fields beyond `UpdateFunc`.
/// PLAYER carries `showSmallerCoins = "Backpack"`, so a round 5 gold still shows all three coins;
/// STATIC does not, so the same amount is gold alone.
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

    // A pure-copper amount under a collapsing type: the leading zeros go, the copper stays.
    s.run("TestPurse.staticMoney = 42 this = TestPurse MoneyFrame_UpdateMoney() this = nil")
        .unwrap();
    assert_eq!(shown(&s, "TestPurse"), (false, false, true));
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `MoneyFrame_Update(frameName, money)` — the family's one **by-name** entry point, and how an
/// addon paints a STATIC frame it just filled without going through an event. It is also where the
/// frame's own width is recomputed from the coins that survived the collapse.
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

    // Every coin is its digits + a 13px icon (MONEY_ICON_WIDTH_SMALL, since this frame is `small`),
    // and the frame is the surviving coins packed with the -4px MONEY_BUTTON_SPACING gap:
    // 13 (base) + 21 + 21 + 21 - (-4) - (-4) = 84, each 21 being one 8px digit plus the icon
    // (2g 3s 4c is a single digit per denomination).
    //
    // **The digits used to contribute 0 here and this test asserted the bare 13s that came of it.**
    // That was the director's cramped gold on a first open: `ShowCoin` sized each coin from a text
    // measure that lands a frame later, so it read 0 every first time. The engine's font
    // measurer answers inside the Lua call that asked now (`script/measure.rs`), so the width
    // is real on the first pass.
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

/// The large template uses the 19px icon and its own spacing — the same painter, the other size
/// constants (`MONEY_ICON_WIDTH` / `MONEY_BUTTON_SPACING`), selected purely by `this.small`.
#[test]
fn the_large_template_measures_with_the_nineteen_pixel_icon() {
    benilla_formats::wow_data_or_skip!();
    let s = harness(20_304);
    // One 8px digit + the 19px icon — the same digit sum as the small template, over the other
    // icon constant. That the two differ ONLY by the icon is what this test is for.
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

/// An unknown type is refused rather than half-applied — the reference's `MoneyFrame_SetType`
/// guard (l.144-147). Its `message()` diagnostic is BasicControls.xml's, which we have not
/// transcribed, so the call is guarded; what must hold either way is that the frame keeps the type
/// it had.
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

/// `SetMoneyFrameColor` recolours the **digits**, not the coin icons — how the reference reddens a
/// price you cannot afford (l.348-356). Read off the extracted quads, which is where "the digits
/// and not the icons" is actually observable.
#[test]
fn set_money_frame_color_recolours_the_digits_and_not_the_icons() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness(12_345); // 1g 23s 45c — three distinguishable digit strings
    s.run("SetMoneyFrameColor(\"TestPurse\", 1.0, 0.1, 0.1)")
        .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s.resolve();
    let quads = s.extract();

    // Both frames show the same purse, so there are six digit quads; exactly the three belonging to
    // the frame we named are red — which also pins that `SetMoneyFrameColor` is scoped by name and
    // does not reach a sibling money frame.
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

    // The coin icons keep their own texture untinted — the half of the reference's contract that a
    // "recolour the whole frame" shortcut would break.
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

/// `MoneyTypeInfo` is read directly by addons, so its shape is part of the contract — all seven
/// reference types, each with an `UpdateFunc`, and the two send-mail rows reading the amounts
/// their engine getters hold (`GetSendMailMoney`/`GetSendMailCOD`, built by 1962).
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
    // The two send-mail rows: with no send-mail amount set, their getters answer 0.
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

/// **`MoneyInputFrame.lua` is `EditBox:SetNumber`'s only caller on the whole chain**, and the verb
/// did not exist until decision 1831 — so the three-box amount editor could not put a number in a
/// box at all. This drives the reference's own file: split an amount, read it back, round-trip.
///
/// The boxes are `numeric="true"`, which is why the value→text law matters here specifically: a
/// non-digit abandons the insert wholesale and leaves the box EMPTY. Every value this path passes
/// is a non-negative integer, so `%.14g` yields plain digits and the gate never trips — but the
/// zero case below is the one that would show if it ever did.
#[test]
fn the_chains_money_input_frame_splits_an_amount_across_its_three_boxes() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = UiScript::new().unwrap();
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua"); // COPPER_PER_GOLD / COPPER_PER_SILVER live here
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    // The manifest's own order: the `.lua` brings the ten verbs, the `.xml` the template.
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

    // Zero is the reference's quiet case: `GetNumber()` on an empty box is already 0, so the
    // equality test short-circuits and `SetNumber` is never reached — the boxes stay EMPTY rather
    // than reading "0", and the total still comes back 0.
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
