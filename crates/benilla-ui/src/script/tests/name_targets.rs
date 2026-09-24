//! **A widget NAME argument is a Lua GLOBAL, not a frame name** — the one rule behind every
//! binding that takes "a frame or its name": `SetPoint`'s `relativeTo`, `SetAllPoints`,
//! `SetParent`, `SetScrollChild`.
//!
//! The client has a single widget namespace and it is `_G`. The wrapper binder `0x701bd0`
//! publishes `_G[name] = T` for a named widget (non-overwriting), and the by-name resolvers read
//! that table back and nothing else — `0x76c760` (FrameScript globals **rawget** + Lua-type-5 +
//! the `vtbl+0x10` tag check) and its geometry-vtable `+0x28` twin `0x76c700`
//! (`_G[name]` → `t[0]` userdata → `IsA`).
//!
//! A frame's published name is therefore only the commonest way such a global comes to exist —
//! **an alias is just as good a name**, and 1.12 addons lean on that. Bartender2 2.0 aliases every
//! stock button (`Bar8Button1 = CharacterBag3Slot`, file scope, `Alias.lua`) and then lays each bar
//! out by anchoring button *i* to the string `"Bar8Button"..(i-1)`. Resolving those against a
//! private frame-name registry finds nothing, every anchor falls back to the parent, and all five
//! bag buttons land in one spot on the bar's far corner. Decision 2105.

use super::common::script;

/// Bartender2's exact shape: stock buttons, addon alias globals that no frame is named, a row laid
/// out through those aliases alone. The row must come out at the addon's pitch.
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

/// The namespace is ONE: a **region** published under a global resolves as a `relativeTo` exactly
/// as a frame does (the real XML anchors frames to FontStrings by name — gossip option rows).
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

/// A global that is not a widget resolves to nothing — the reference's type-5 + tag check — and
/// the binding then takes its unresolved-name leg, which **raises** (`0x87ccd4`; decision 2176).
/// The value is never used, and no anchor is left behind.
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

/// **`$parent` is expanded at RUNTIME, on the anchor path only.** `0x76c5b0` has exactly two call
/// sites — `SetName 0x76c691` and `0x76c71c` *inside* the layout resolver `0x76c700` — so
/// `SetPoint`/`SetAllPoints` run the token against the anchoring frame's first **named** ancestor
/// (skipping anonymous links, `"Top"` when there is none), while `SetParent` and `SetScrollChild`,
/// which call `0x76c760` directly, do not.
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

/// The other half of the same fact: the reparent bindings never see the token, so a
/// `$parent`-prefixed name reaches them literally and fails to resolve.
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

/// `SetParent` takes the same namespace (`0x7a1550`'s NAME-string path → `0x76c760`) — Bartender2
/// re-parents every stock button by alias before it anchors it.
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

/// `SetAllPoints` shares the resolver, so it shares the rule.
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

/// **A leading `$parent` in a Lua constructor's NAME argument is expanded**, exactly as in XML —
/// `CreateFrame 0x7060b0`, `CreateTexture 0x773a20` and `CreateFontString 0x773c30` all build a
/// synthetic node carrying `name=` and read it back through `CScriptRegion::SetName 0x76c650`,
/// which is one of the expander `0x76c5b0`'s two call sites.
///
/// Found by decision 2176's raise, which is the point of a raise: the unexpanded name had been
/// there all along, degrading quietly into a parent-anchored fallback. `FonzAppraiser` names
/// roughly thirty widgets this way and `pfQuest/browser.lua:723` builds its search icon as
/// `input:CreateTexture("$parentSearchIcon", "OVERLAY")`.
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
    // …and the expanded name is what reached `_G`, which is the only widget namespace (2105).
    assert_eq!(
        s.eval::<String>("return RootKid:GetName()").unwrap(),
        "RootKid"
    );
    assert!(
        s.eval::<bool>(r#"return getglobal("$parentKid") == nil"#)
            .unwrap(),
        "the raw token names nothing"
    );
    // A region's `$parent` is its OWNER frame, so the walk starts one link lower.
    assert_eq!(
        s.eval::<String>("return RootKidIcon:GetName()").unwrap(),
        "RootKidIcon"
    );
    // The payoff: an anchor by the built name resolves instead of raising.
    s.run(r#"Icon:SetPoint("TOPLEFT", "RootKid", "TOPLEFT", 0, 0)"#)
        .unwrap();
}
