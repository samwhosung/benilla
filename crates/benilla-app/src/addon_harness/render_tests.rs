//! Tests for the render column, in both directions: synthetic fixtures that paint a window, an
//! overlay or nothing, and two real corpus addons with known opposite outcomes.

use std::path::{Path, PathBuf};

use super::{survey, Drew};

/// One throwaway AddOns root, cleaned up on drop even if a test panics.
struct Fixtures(PathBuf);

impl Fixtures {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "benilla-render-{tag}-{}-{:?}",
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

/// An addon that paints registers, one that does not stays `nothing`, and creating a frame is not
/// painting.
#[test]
fn the_render_column_can_fail() {
    benilla_formats::wow_data_or_skip!();
    let fx = Fixtures::new("cannotfail");
    // Paints: a texture on a window of its own.
    fx.addon(
        "PaintsAWindow",
        r#"
        local f = CreateFrame("Frame", "PaintsAWindowFrame", UIParent)
        f:SetWidth(64) f:SetHeight(64)
        f:SetPoint("CENTER", UIParent, "CENTER", 0, 0)
        local t = f:CreateTexture("PaintsAWindowTexture", "ARTWORK")
        t:SetAllPoints(f)
        t:SetTexture("Interface\\Icons\\INV_Misc_Bag_08")
        f:Show()
    "#,
    );
    // Paints, but only onto a pre-existing frame.
    fx.addon(
        "PaintsOnOurs",
        r#"
        local host = ActionButton1 or UIParent
        local fs = host:CreateFontString("PaintsOnOursText", "OVERLAY", "GameFontNormal")
        fs:SetPoint("CENTER", host, "CENTER", 0, 0)
        fs:SetText("hooked")
        fs:Show()
    "#,
    );
    // Creates and shows a frame, whose own `QuadContent::Frame` slot paints nothing.
    fx.addon(
        "DrawsNothing",
        r#"
        DrawsNothingRan = true
        local f = CreateFrame("Frame", "DrawsNothingFrame", UIParent)
        f:SetWidth(64) f:SetHeight(64)
        f:SetPoint("CENTER", UIParent, "CENTER", 0, 0)
        f:Show()
    "#,
    );

    let reports = survey(fx.root());
    let drew = |name: &str| {
        reports
            .iter()
            .find(|r| r.name == name)
            .unwrap_or_else(|| panic!("{name} was not surveyed"))
    };

    // Each fixture must load, or "drew nothing" would measure a load failure.
    for name in ["PaintsAWindow", "PaintsOnOurs", "DrawsNothing"] {
        assert!(
            drew(name).loaded,
            "{name} must load clean or this test proves nothing: {:?}",
            drew(name).errors
        );
    }

    assert_eq!(
        drew("PaintsAWindow").render.drew(),
        Drew::Own,
        "a texture on a frame of its own is a window of its own"
    );
    assert!(
        drew("PaintsAWindow")
            .render
            .frames
            .iter()
            .any(|f| f == "PaintsAWindowFrame"),
        "…and the row names the frame it came from: {:?}",
        drew("PaintsAWindow").render.frames
    );
    assert_eq!(
        drew("PaintsOnOurs").render.drew(),
        Drew::Overlay,
        "a region hung off one of OUR frames is an overlay, not a window"
    );
    assert_eq!(
        drew("DrawsNothing").render.drew(),
        Drew::Nothing,
        "creating and showing a frame paints nothing — a frame's own draw slot is dropped by the \
         renderer, and an implementation that counted quads rather than PAINTING quads would call \
         this a pass"
    );
    assert_eq!(
        (
            drew("DrawsNothing").render.own_quads,
            drew("DrawsNothing").render.overlay_quads
        ),
        (0, 0)
    );
}

/// `!OmniCC` paints through an anonymous frame parented to an existing cooldown, an overlay;
/// Bagnon builds its own window of item-slot buttons.
#[test]
fn omnicc_and_bagnon_come_out_on_opposite_sides() {
    benilla_formats::wow_data_or_skip!();
    // A skip the gate can refuse (`benilla_formats::install`).
    let corpus = benilla_formats::addon_corpus_or_skip!();
    // Symlinked into a small root: the full corpus sweep is the `addon_harness` example's job.
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

    // The positive control.
    assert_eq!(
        row("!OmniCC").render.drew(),
        Drew::Overlay,
        "!OmniCC's countdown text draws in the live client; it paints via an ANONYMOUS frame \
         parented to a cooldown of ours, which is precisely what a name-based check cannot see"
    );

    assert_eq!(
        row("Bagnon").render.drew(),
        Drew::Own,
        "Bagnon draws its own inventory window"
    );
    assert!(
        row("Bagnon")
            .render
            .frames
            .iter()
            .any(|f| f.starts_with("BagnonItem")),
        "…and the slots are what it draws — the item slots a player sees: {:?}",
        row("Bagnon").render.frames
    );
}
