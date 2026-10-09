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
        f:SetScript("OnUpdate", function() local self, elapsed = this, arg1 ticks = ticks + 1; last = elapsed end)
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

/// The show cascade `0x76ae10` walks its children (`0x76aed5`) before its own notify
/// (`0x76aef5`), and the hide cascade `0x76ad50` likewise (`0x76adee`, then `0x76adfd`): both are
/// post-order, depth-first in child-link order.
#[test]
fn a_cascade_notifies_children_before_their_parent() {
    let s = script();
    s.run(
        r#"
        LOG = {}
        local function log(tag) return function() table.insert(LOG, tag) end end
        local p = CreateFrame("Frame", "PostP")
        local c1 = CreateFrame("Frame", "PostC1", p)
        local g = CreateFrame("Frame", "PostG", c1)
        local c2 = CreateFrame("Frame", "PostC2", p)
        for f, tag in pairs({ [p] = "p", [c1] = "c1", [g] = "g", [c2] = "c2" }) do
            f:SetScript("OnShow", log(tag .. "+"))
            f:SetScript("OnHide", log(tag .. "-"))
        end
        p:Hide()
        p:Show()
    "#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<String>("return table.concat(LOG, ',')").unwrap(),
        "g-,c1-,c2-,p-,g+,c1+,c2+,p+"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The child walk follows live links and each child's show re-checks its own gate when the walk
/// reaches it (`0x76ae1d`): a sibling an earlier sibling's OnShow hides gets no OnShow, and, never
/// having become visible, no OnHide either.
#[test]
fn a_sibling_hidden_by_an_earlier_siblings_onshow_gets_no_onshow() {
    let s = script();
    s.run(
        r#"
        LOG = {}
        local p = CreateFrame("Frame", "LiveP")
        local c1 = CreateFrame("Frame", "LiveC1", p)
        local c2 = CreateFrame("Frame", "LiveC2", p)
        p:Hide()
        c1:SetScript("OnShow", function() table.insert(LOG, "c1+") LiveC2:Hide() end)
        c2:SetScript("OnShow", function() table.insert(LOG, "c2+") end)
        c2:SetScript("OnHide", function() table.insert(LOG, "c2-") end)
        p:SetScript("OnShow", function() table.insert(LOG, "p+") end)
        p:Show()
    "#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<String>("return table.concat(LOG, ',')").unwrap(),
        "c1+,p+"
    );
    assert!(!s.eval::<bool>("return LiveC2:IsVisible() == 1").unwrap());
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A sibling an earlier sibling's OnHide hides runs its whole hide, OnHide included, nested in that
/// handler (`0x7758bb`); when the parent's walk reaches it, it is already invisible and the hide
/// gate (`0x76ad5b`) returns without a second OnHide. The parent's own OnHide comes last.
#[test]
fn a_sibling_hidden_by_an_earlier_siblings_onhide_is_notified_once_inside_it() {
    let s = script();
    s.run(
        r#"
        LOG = {}
        local p = CreateFrame("Frame", "NestP")
        local c1 = CreateFrame("Frame", "NestC1", p)
        local c2 = CreateFrame("Frame", "NestC2", p)
        c1:SetScript("OnHide", function()
            table.insert(LOG, "c1-")
            NestC2:Hide()
            table.insert(LOG, "c1-end")
        end)
        c2:SetScript("OnHide", function() table.insert(LOG, "c2-") end)
        p:SetScript("OnHide", function() table.insert(LOG, "p-") end)
        p:Hide()
    "#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<String>("return table.concat(LOG, ',')").unwrap(),
        "c1-,c2-,c1-end,p-"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A child's OnShow that hides the parent: the parent's hide cascade runs nested (the child's
/// OnHide, then the parent's), the next sibling then fails its parent gate (`0x76ae35`) and is
/// never shown, and the parent's own OnShow still fires last, since nothing between the walk and
/// `0x76aef5` re-checks.
#[test]
fn a_parent_hidden_by_a_childs_onshow_stops_the_walk_but_still_gets_its_onshow() {
    let s = script();
    s.run(
        r#"
        LOG = {}
        local p = CreateFrame("Frame", "StopP")
        local c1 = CreateFrame("Frame", "StopC1", p)
        local c2 = CreateFrame("Frame", "StopC2", p)
        p:Hide()
        c1:SetScript("OnShow", function() table.insert(LOG, "c1+") StopP:Hide() end)
        c1:SetScript("OnHide", function() table.insert(LOG, "c1-") end)
        c2:SetScript("OnShow", function() table.insert(LOG, "c2+") end)
        p:SetScript("OnShow", function() table.insert(LOG, "p+") end)
        p:SetScript("OnHide", function() table.insert(LOG, "p-") end)
        p:Show()
    "#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<String>("return table.concat(LOG, ',')").unwrap(),
        "c1+,c1-,p-,p+"
    );
    assert!(!s.eval::<bool>("return StopC2:IsVisible() == 1").unwrap());
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The Lua `Show` writes the shown bit and dispatches the cascade unconditionally (`0x7757f1`,
/// `0x7757fb`), so a frame whose bit is already set but which is not visible (a `SetParent` from a
/// hidden parent to a shown one skips the show half, `0x76abe3`) becomes visible on `Show()`.
#[test]
fn show_on_a_shown_but_invisible_frame_makes_it_visible() {
    let s = script();
    s.run(
        r#"
        SHOWS = 0
        local hidden = CreateFrame("Frame", "StaleHidden")
        hidden:Hide()
        local shown = CreateFrame("Frame", "StaleShown")
        local f = CreateFrame("Frame", "Stale", hidden)
        f:SetScript("OnShow", function() SHOWS = SHOWS + 1 end)
        f:SetParent(shown)
    "#,
    )
    .unwrap();
    assert!(
        !s.eval::<bool>("return Stale:IsVisible() == 1").unwrap(),
        "the reparent from an invisible parent runs no show half"
    );
    s.run("Stale:Show()").unwrap();
    assert!(s.eval::<bool>("return Stale:IsVisible() == 1").unwrap());
    assert_eq!(s.eval::<i64>("return SHOWS").unwrap(), 1);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}
