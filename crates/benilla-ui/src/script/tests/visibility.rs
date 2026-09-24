//! Show and Hide fire OnShow/OnHide only on effective transitions; OnUpdate runs only when visible.

use super::common::script;

#[test]
fn show_hide_fires_only_on_effective_transitions() {
    let s = script();
    s.run(
        r#"
        shows, hides = 0, 0
        local f = CreateFrame("Frame", "Vis")
        f:SetScript("OnShow", function() shows = shows + 1 end)
        f:SetScript("OnHide", function() hides = hides + 1 end)
        f:Hide()   -- true→false: OnHide (1)
        f:Hide()   -- no transition: nothing
        f:Show()   -- false→true: OnShow (1)
        f:Show()   -- no transition: nothing
    "#,
    )
    .unwrap();
    let (shows, hides): (i64, i64) = s.eval("return shows, hides").unwrap();
    assert_eq!((shows, hides), (1, 1));
    assert!(s.errors().is_empty(), "no script errors: {:?}", s.errors());
}

#[test]
fn mid_tree_hide_fires_onhide_for_the_subtree() {
    let s = script();
    s.run(
        r#"
        childhides = 0
        local p = CreateFrame("Frame", "PH")
        local c = CreateFrame("Frame", "CH", p)
        c:SetScript("OnHide", function() childhides = childhides + 1 end)
        p:Hide()   -- child loses effective visibility though its own shown stays true
    "#,
    )
    .unwrap();
    let hides: i64 = s.eval("return childhides").unwrap();
    assert_eq!(hides, 1);
}

#[test]
fn tick_runs_onupdate_only_when_effectively_visible() {
    let mut s = script();
    s.run(
        r#"
        ticks, last = 0, 0
        local f = CreateFrame("Frame", "UF")
        f:SetScript("OnUpdate", function(self, elapsed) ticks = ticks + 1; last = elapsed end)
    "#,
    )
    .unwrap();

    s.tick(0.25);
    assert_eq!(s.eval::<i64>("return ticks").unwrap(), 1);
    assert!((s.eval::<f64>("return last").unwrap() - 0.25).abs() < 1e-6);

    s.run("UF:Hide()").unwrap();
    s.tick(0.25); // hidden → no fire
    assert_eq!(s.eval::<i64>("return ticks").unwrap(), 1);

    s.run("UF:Show()").unwrap();
    s.tick(0.25); // visible again → fires
    assert_eq!(s.eval::<i64>("return ticks").unwrap(), 2);
}
