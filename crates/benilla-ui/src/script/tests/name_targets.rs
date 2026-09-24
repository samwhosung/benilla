//! A widget name argument (`SetPoint`'s `relativeTo`, `SetAllPoints`, `SetParent`,
//! `SetScrollChild`) is a Lua global, so an alias resolves too: the binder `0x701bd0` publishes
//! `_G[name]` without overwriting, and the resolvers `0x76c760` (rawget, type 5, `vtbl+0x10` tag)
//! and `0x76c700` (`_G[name]`, `t[0]`, `IsA`) read `_G` and nothing else.

use super::common::script;

/// Bartender2's shape: a row laid out through alias globals that are no frame's own name.
#[test]
fn setpoint_relative_to_resolves_an_alias_global() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        -- The reference UI's own frames.
        Bar = CreateFrame("Frame", "Bar")
        Bar:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 100, 50)
        Bar:SetWidth(200); Bar:SetHeight(40)
        for i = 1, 5 do
            local b = CreateFrame("Button", "StockButton" .. i, Bar)
            b:SetWidth(30); b:SetHeight(30)
        end

        -- The addon's aliases: plain globals, no frame's name (Bartender2's Alias.lua).
        Alias1 = StockButton1
        Alias2 = StockButton2
        Alias3 = StockButton3
        Alias4 = StockButton4
        Alias5 = StockButton5

        -- ...and the layout it drives through them (Bartender2's SetupBar8).
        for i = 1, 5 do getglobal("Alias" .. i):ClearAllPoints() end
        Alias1:SetPoint("BOTTOMLEFT", "Bar", "BOTTOMLEFT", 5, 5)
        for i = 2, 5 do
            getglobal("Alias" .. i):SetPoint("BOTTOMLEFT", "Alias" .. (i - 1), "BOTTOMRIGHT", 2, 0)
        end
    "#,
    )
    .unwrap();
    s.resolve();

    // Bar's BOTTOMLEFT is (100, 50); button 1 sits at +(5, 5) and each next one 30 + 2 further on.
    for i in 1..=5 {
        let left: f32 = s
            .eval(&format!("return StockButton{i}:GetLeft()"))
            .unwrap_or_else(|e| panic!("StockButton{i}:GetLeft(): {e}"));
        let bottom: f32 = s
            .eval(&format!("return StockButton{i}:GetBottom()"))
            .unwrap();
        assert_eq!(
            left,
            105.0 + 32.0 * (i - 1) as f32,
            "button {i} must sit one pitch past its predecessor, not stacked on the bar"
        );
        assert_eq!(bottom, 55.0, "the row shares one baseline");
    }
    assert!(
        s.take_warnings().is_empty(),
        "an alias global is a resolvable target, not a miss"
    );
}

/// One namespace: stock XML anchors frames to FontStrings by name (the gossip option rows).
#[test]
fn setpoint_relative_to_resolves_a_region_alias_global() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        Panel = CreateFrame("Frame", "Panel")
        Panel:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 0, 0)
        Panel:SetWidth(400); Panel:SetHeight(300)
        local tex = Panel:CreateTexture("PanelSwatch", "ARTWORK")
        tex:SetPoint("BOTTOMLEFT", "Panel", "BOTTOMLEFT", 10, 20)
        tex:SetWidth(50); tex:SetHeight(50)
        Swatch = PanelSwatch          -- the alias

        Tag = CreateFrame("Frame", "Tag", Panel)
        Tag:SetWidth(10); Tag:SetHeight(10)
        Tag:SetPoint("BOTTOMLEFT", "Swatch", "BOTTOMRIGHT", 4, 0)
    "#,
    )
    .unwrap();
    s.resolve();

    let left: f32 = s.eval("return Tag:GetLeft()").unwrap();
    let bottom: f32 = s.eval("return Tag:GetBottom()").unwrap();
    assert_eq!(
        left, 64.0,
        "10 + 50 + 4 — anchored to the texture, not to Panel"
    );
    assert_eq!(bottom, 20.0);
}

/// The type-5 and tag check reject it, and the unresolved-name leg raises (`0x87ccd4`).
#[test]
fn setpoint_relative_to_ignores_a_non_widget_global() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        Host = CreateFrame("Frame", "Host")
        Host:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 10, 10)
        Host:SetWidth(100); Host:SetHeight(100)
        NotAWidget = 5
        Child = CreateFrame("Frame", "Child", Host)
        Child:SetWidth(20); Child:SetHeight(20)
    "#,
    )
    .unwrap();
    let e = s
        .run(r#"Child:SetPoint("BOTTOMLEFT", "NotAWidget", "BOTTOMRIGHT", 0, 0)"#)
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("Child:SetPoint(): Couldn't find region named 'NotAWidget'"),
        "a global holding a non-widget names nothing: {e}"
    );
    let n: i64 = s.eval("return Child:GetNumPoints()").unwrap();
    assert_eq!(n, 0, "the raise leaves no anchor behind");
}

/// The expander `0x76c5b0` is called only from `SetName` (`0x76c691`) and the layout resolver
/// (`0x76c71c` in `0x76c700`), so anchors expand `$parent` against the first named ancestor
/// (`"Top"` if none); `SetParent` and `SetScrollChild` call `0x76c760` directly and do not.
#[test]
fn setpoint_relative_to_expands_parent_against_the_first_named_ancestor() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        Root = CreateFrame("Frame", "Root")
        Root:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 0, 0)
        Root:SetWidth(300); Root:SetHeight(200)

        Peg = CreateFrame("Frame", "RootPeg", Root)
        Peg:SetPoint("BOTTOMLEFT", "Root", "BOTTOMLEFT", 20, 30)
        Peg:SetWidth(10); Peg:SetHeight(10)

        -- An ANONYMOUS link in between: the walk skips it and lands on Root.
        Anon = CreateFrame("Frame", nil, Root)
        Anon:SetAllPoints(Root)
        Kid = CreateFrame("Frame", "Kid", Anon)
        Kid:SetWidth(5); Kid:SetHeight(5)
        Kid:SetPoint("BOTTOMLEFT", "$parentPeg", "BOTTOMRIGHT", 0, 0)
    "#,
    )
    .unwrap();
    s.resolve();
    let (l, b): (f32, f32) = s.eval("return Kid:GetLeft(), Kid:GetBottom()").unwrap();
    assert_eq!(
        (l, b),
        (30.0, 30.0),
        "`$parentPeg` is RootPeg (20 + its 10 wide), not an unresolvable literal"
    );
    assert!(s.take_warnings().is_empty());
}

#[test]
fn setparent_does_not_expand_parent() {
    let s = script();
    s.run(
        r#"
        Root = CreateFrame("Frame", "Root")
        RootPeg = CreateFrame("Frame", "RootPeg", Root)
        Kid = CreateFrame("Frame", "Kid", Root)
    "#,
    )
    .unwrap();
    let err = s
        .eval::<()>(r#"Kid:SetParent("$parentPeg")"#)
        .expect_err("an unexpanded token names nothing");
    assert!(
        err.to_string().contains("$parentPeg"),
        "the raise quotes the name as passed: {err}"
    );
}

/// `SetParent`'s name path (`0x7a1550`) resolves through `0x76c760` too.
#[test]
fn setparent_resolves_an_alias_global() {
    let s = script();
    s.run(
        r#"
        Holder = CreateFrame("Frame", "Holder")
        Moved = CreateFrame("Frame", "Moved")
        HolderAlias = Holder
        Moved:SetParent("HolderAlias")
    "#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<String>("return Moved:GetParent():GetName()")
            .unwrap(),
        "Holder"
    );
}

#[test]
fn setallpoints_resolves_an_alias_global() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        Plate = CreateFrame("Frame", "Plate")
        Plate:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 30, 40)
        Plate:SetWidth(120); Plate:SetHeight(60)
        PlateAlias = Plate
        Overlay = CreateFrame("Frame", "Overlay")
        Overlay:SetAllPoints("PlateAlias")
    "#,
    )
    .unwrap();
    s.resolve();
    let (l, b, w, h): (f32, f32, f32, f32) = s
        .eval(
            "return Overlay:GetLeft(), Overlay:GetBottom(), Overlay:GetWidth(), Overlay:GetHeight()",
        )
        .unwrap();
    assert_eq!((l, b, w, h), (30.0, 40.0, 120.0, 60.0));
}

/// `CreateFrame 0x7060b0`, `CreateTexture 0x773a20` and `CreateFontString 0x773c30` read their
/// `name=` back through `CScriptRegion::SetName 0x76c650`, which calls the expander `0x76c5b0`.
#[test]
fn a_constructor_name_expands_parent() {
    let s = script();
    s.run(
        r#"
        Root = CreateFrame("Frame", "Root")
        Mid = CreateFrame("Frame", nil, Root)          -- anonymous: the walk skips it
        Kid = CreateFrame("Frame", "$parentKid", Mid)
        Icon = Kid:CreateTexture("$parentIcon", "OVERLAY")
        "#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<String>("return Kid:GetName()").unwrap(),
        "RootKid",
        "the base is the first NAMED ancestor of the frame's parent, not the parent itself"
    );
    // The expanded name is what reached `_G`.
    assert_eq!(
        s.eval::<String>("return RootKid:GetName()").unwrap(),
        "RootKid"
    );
    assert!(
        s.eval::<bool>(r#"return getglobal("$parentKid") == nil"#)
            .unwrap(),
        "the raw token names nothing"
    );
    // A region's `$parent` is its owner frame.
    assert_eq!(
        s.eval::<String>("return RootKidIcon:GetName()").unwrap(),
        "RootKidIcon"
    );
    // An anchor by the built name resolves.
    s.run(r#"Icon:SetPoint("TOPLEFT", "RootKid", "TOPLEFT", 0, 0)"#)
        .unwrap();
}
