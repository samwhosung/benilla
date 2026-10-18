//! Frame-API basics: `GetLocale`, `SetAllPoints` and `GetPoint` readback.

use super::common::script;

#[test]
fn get_locale_and_set_all_points() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    // `GetLocale`: enUS, index 0, until the host seats the install's locale.
    assert_eq!(s.eval::<String>("return GetLocale()").unwrap(), "enUS");

    s.run(
        r#"
        p = CreateFrame("Frame", "SAP_Parent")
        p:SetPoint("BOTTOMLEFT", 100, 100); p:SetWidth(200); p:SetHeight(50)
        c = CreateFrame("Frame", "SAP_Child", p)
        c:SetAllPoints()                          -- default: the parent
        f = CreateFrame("Frame", "SAP_Free")
        f:SetAllPoints("SAP_Parent")              -- by name
    "#,
    )
    .unwrap();
    s.resolve();

    let ok: bool = s
        .eval(
            r#"
        local function near(a, b) return math.abs(a - b) < 1e-3 end -- the spans through the law
        return near(c:GetWidth(), 200) and near(c:GetHeight(), 50)
           and near(f:GetWidth(), 200) and near(f:GetHeight(), 50)
    "#,
        )
        .unwrap();
    assert!(ok, "SetAllPoints matched the target rect");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

#[test]
fn getpoint_reads_back_anchors() {
    let s = script();
    s.run(
        r#"
        a = CreateFrame("Frame", "GP_A")
        a:SetPoint("BOTTOMLEFT", 100, 100)
        b = CreateFrame("Frame", "GP_B")
        b:SetPoint("TOPLEFT", a, "BOTTOMRIGHT", 4, -8)
    "#,
    )
    .unwrap();
    // Screen-anchored: `relativeTo` reads nil.
    let ok: bool = s
        .eval(
            r#"
        local p, rel, rp, x, y = GP_A:GetPoint()
        -- The offsets read back as `G(S(100))`, 99.99999850988391 at 4:3.
        return p == "BOTTOMLEFT" and rel == nil and rp == "BOTTOMLEFT"
            and x == 99.99999850988391 and y == 99.99999850988391
    "#,
        )
        .unwrap();
    assert!(ok, "screen anchor reads back");
    // Frame-anchored: `relativeTo` is the same wrapper table.
    let ok: bool = s
        .eval(
            r#"
        local p, rel, rp, x, y = GP_B:GetPoint(1)
        return p == "TOPLEFT" and rel == a and rp == "BOTTOMRIGHT" and x == 4 and y == -8
    "#,
        )
        .unwrap();
    assert!(ok, "frame anchor reads back with wrapper identity");
    // Out-of-range n: all nils.
    assert!(s.eval::<bool>("return GP_B:GetPoint(9) == nil").unwrap());
}
