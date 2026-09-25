//! Tests for the use column: one fixture per gesture that raises from exactly one handler, a
//! silent button that must survive with `driven >= 1`, a painted mouse-disabled frame that must be
//! `untouched`, and two real corpus addons that must be reachable. The fixtures pin each gesture,
//! not each line: removing only the hover move or only the explicit left click stays green, since
//! the drag also hovers and clicks.

use std::path::{Path, PathBuf};

use super::{survey, Used};

/// One throwaway AddOns root, cleaned up on drop even if a test panics; its own temp prefix keeps
/// it apart from `render_tests`'.
struct Fixtures(PathBuf);

impl Fixtures {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "benilla-use-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
    }

    /// Write `<root>/<name>/{<name>.toc, body.lua}`.
    fn addon(&self, name: &str, body: &str) -> &Self {
        let dir = self.0.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{name}.toc")),
            "## Interface: 11200\nbody.lua\n",
        )
        .unwrap();
        std::fs::write(dir.join("body.lua"), body).unwrap();
        self
    }

    fn root(&self) -> &Path {
        &self.0
    }
}

impl Drop for Fixtures {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A 64×64 painted Button at the centre of the screen, plus whatever `extra` wires onto it.
/// Painted, since the use column aims at the render column's quads; centred, clear of the default
/// UI's windows.
fn painted_button(extra: &str) -> String {
    format!(
        r#"
        local f = CreateFrame("Button", "FixtureButton", UIParent)
        f:SetWidth(64) f:SetHeight(64)
        f:SetPoint("CENTER", UIParent, "CENTER", 0, 0)
        local t = f:CreateTexture("FixtureButtonIcon", "ARTWORK")
        t:SetAllPoints(f)
        t:SetTexture("Interface\\Icons\\INV_Misc_Bag_08")
        f:Show()
        {extra}
    "#
    )
}

/// Every gesture is driven, silence stays silent, and nothing touched is not a pass.
#[test]
fn the_use_column_can_fail() {
    let fx = Fixtures::new("cannotfail");
    // One fixture per gesture, each raising from exactly one handler.
    fx.addon(
        "RaisesOnEnter",
        &painted_button(r#"f:SetScript("OnEnter", function() error("USEFIXTURE_ENTER") end)"#),
    );
    fx.addon(
        "RaisesOnLeftClick",
        &painted_button(r#"f:SetScript("OnClick", function() error("USEFIXTURE_LEFTCLICK") end)"#),
    );
    // Registered for `RightButtonUp` only (the default is `{"LeftButtonUp"}`), so no left click
    // reaches it.
    fx.addon(
        "RaisesOnRightClick",
        &painted_button(
            r#"
            f:RegisterForClicks("RightButtonUp")
            f:SetScript("OnClick", function() error("USEFIXTURE_RIGHTCLICK") end)
        "#,
        ),
    );
    // `OnDragStart` fires only past the 4 px threshold, so the drag must actually move.
    fx.addon(
        "RaisesOnDrag",
        &painted_button(
            r#"
            f:RegisterForDrag("LeftButton")
            f:SetScript("OnDragStart", function() error("USEFIXTURE_DRAG") end)
        "#,
        ),
    );
    // The same button with no handlers: driven and silent, so the errors above are the addons'.
    fx.addon("SilentButTouchable", &painted_button(""));
    // Paints but takes no mouse: `untouched`, never `ok`.
    fx.addon(
        "PaintsButTakesNoMouse",
        r#"
        local f = CreateFrame("Frame", "UntouchableFrame", UIParent)
        f:SetWidth(64) f:SetHeight(64)
        f:SetPoint("CENTER", UIParent, "CENTER", 0, 0)
        f:EnableMouse(false)
        local t = f:CreateTexture("UntouchableIcon", "ARTWORK")
        t:SetAllPoints(f)
        t:SetTexture("Interface\\Icons\\INV_Misc_Bag_08")
        f:Show()
    "#,
    );

    let reports = survey(fx.root());
    let row = |name: &str| {
        reports
            .iter()
            .find(|r| r.name == name)
            .unwrap_or_else(|| panic!("{name} was not surveyed"))
    };

    // Each fixture must load and draw, or `driven = 0` would measure something else.
    for name in [
        "RaisesOnEnter",
        "RaisesOnLeftClick",
        "RaisesOnRightClick",
        "RaisesOnDrag",
        "SilentButTouchable",
        "PaintsButTakesNoMouse",
    ] {
        let r = row(name);
        assert!(
            r.loaded,
            "{name} must load clean or this test proves nothing: {:?}",
            r.errors
        );
        assert_ne!(
            r.render.drew(),
            super::Drew::Nothing,
            "{name} must paint, or the use probe has nothing to aim at"
        );
    }

    // ── It reports, per gesture ──
    for (name, marker) in [
        ("RaisesOnEnter", "USEFIXTURE_ENTER"),
        ("RaisesOnLeftClick", "USEFIXTURE_LEFTCLICK"),
        ("RaisesOnRightClick", "USEFIXTURE_RIGHTCLICK"),
        ("RaisesOnDrag", "USEFIXTURE_DRAG"),
    ] {
        let r = row(name);
        assert_eq!(
            r.used.verdict(),
            Used::Raised,
            "{name} raises from the one handler its gesture reaches; a column that does not drive \
             that gesture reports it clean. driven={} errors={:?}",
            r.used.driven,
            r.used.errors
        );
        assert!(
            r.used.errors.iter().any(|e| e.contains(marker)),
            "{name}'s error must be ITS error, not something the probe stirred up elsewhere: {:?}",
            r.used.errors
        );
    }

    // ── It stays quiet when nothing raises ──
    let quiet = row("SilentButTouchable");
    assert_eq!(
        quiet.used.verdict(),
        Used::Survived,
        "a painted, mouse-taking button with no handlers must survive being used: {:?}",
        quiet.used.errors
    );
    assert!(
        quiet.used.driven >= 1,
        "…and it must actually have been DRIVEN — a `Survived` with driven=0 is the failure this \
         whole column is about"
    );

    // ── Nothing touched is not a pass ──
    let untouched = row("PaintsButTakesNoMouse");
    assert_eq!(
        untouched.used.verdict(),
        Used::Untouched,
        "a frame that paints and takes no mouse offers the probe nothing; that is not a pass"
    );
    assert_eq!(
        (untouched.used.driven, untouched.used.touchable),
        (0, 0),
        "…and it says so in the numbers, not only in the verdict"
    );
}

/// `!OmniCC` is never `raised`: its output is a `FontString` on an anonymous mouse-disabled frame,
/// so it is `untouched`. Bagnon's item slots must be reachable (`driven >= 1` on a `BagnonItem*`
/// frame); whether they raise is not asserted here.
#[test]
fn the_directors_two_verified_addons_are_reachable_and_omnicc_is_not_broken() {
    benilla_formats::wow_data_or_skip!();
    // A skip the gate can refuse (`benilla_formats::install`).
    let corpus = benilla_formats::addon_corpus_or_skip!();
    let fx = Fixtures::new("oracle");
    for name in ["!OmniCC", "Bagnon", "Bagnon_Core", "Bagnon_Forever"] {
        std::os::unix::fs::symlink(corpus.join(name), fx.root().join(name)).unwrap();
    }
    let reports = survey(fx.root());
    let row = |name: &str| {
        reports
            .iter()
            .find(|r| r.name == name)
            .unwrap_or_else(|| panic!("{name} is not in the corpus"))
    };

    let omni = row("!OmniCC");
    assert_ne!(
        omni.used.verdict(),
        Used::Raised,
        "!OmniCC is on the director's screen and works; a column that calls it broken is broken \
         itself: {:?}",
        omni.used.errors
    );

    let bagnon = row("Bagnon");
    assert!(
        bagnon.used.driven >= 1,
        "Bagnon draws bag slots the director can put a cursor on; the probe must be able to reach \
         at least one (drew={:?} touchable={})",
        bagnon.render.frames,
        bagnon.used.touchable
    );
    assert!(
        bagnon
            .used
            .frames
            .iter()
            .any(|f| f.starts_with("BagnonItem")),
        "…and the thing it touched must be a SLOT — the exact widget the director hovered: {:?}",
        bagnon.used.frames
    );
}
