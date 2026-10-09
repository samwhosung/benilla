//! The UI scale through the real UI pass: `uiScale` is `UIParent`'s own frame scale (`0x494550`
//! calls `0x76ac10` on the frame named `"UIParent"`), and the screen root under it keeps scale 1
//! and is 768 units tall, so a frame outside `UIParent` is not scaled by it.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use benilla_ui::script::UiScript;

use super::extract::{paint_script, tick_script};
use crate::portrait::PortraitImages;
use crate::ui_pass::{UiQuad, UiQuads};

/// A `UIParent` filling the root with a red 100-unit square inside it, 10 units in from its top
/// left, and a green 100-unit square at the root's bottom left with no parent.
const TREE: &str = r#"
    local ui = CreateFrame("Frame", "UIParent")
    ui:SetPoint("BOTTOMLEFT", 0, 0)
    ui:SetPoint("TOPRIGHT", 0, 0)
    local inside = CreateFrame("Frame", "Inside", UIParent)
    inside:SetPoint("TOPLEFT", 10, -10)
    inside:SetWidth(100); inside:SetHeight(100)
    local red = inside:CreateTexture(nil, "ARTWORK")
    red:SetTexture(1, 0, 0)
    red:SetAllPoints()
    local outside = CreateFrame("Frame", "Outside")
    outside:SetPoint("BOTTOMLEFT", 0, 0)
    outside:SetWidth(100); outside:SetHeight(100)
    local green = outside:CreateTexture(nil, "ARTWORK")
    green:SetTexture(0, 1, 0)
    green:SetAllPoints()
"#;

/// Both halves of the UI pass over [`TREE`] on a `w`×`h` window, the dial at `ui_scale`.
fn app(w: u32, h: u32, ui_scale: f32) -> App {
    let script = UiScript::new().unwrap();
    script.run(TREE).unwrap();
    let mut app = App::new();
    app.insert_non_send_resource(script);
    app.init_resource::<UiQuads>();
    app.init_resource::<super::UiPassState>();
    app.init_resource::<Assets<Image>>();
    app.init_resource::<PortraitImages>();
    app.init_resource::<crate::portrait::BoothPanes>();
    app.init_resource::<crate::ui_models::UiModelTiles>();
    app.init_resource::<crate::minimap::MinimapWidget>();
    app.init_resource::<super::UiFrameCost>();
    app.init_resource::<super::UiCostWanted>();
    app.init_resource::<Time>();
    app.init_resource::<Time<Real>>();
    app.init_resource::<super::UiClock>();
    app.insert_resource(super::UiScaleCvar(ui_scale));
    app.world_mut().spawn((
        Window {
            resolution: UVec2::new(w, h).into(),
            ..default()
        },
        PrimaryWindow,
    ));
    app.add_systems(Update, (tick_script, paint_script).chain());
    app.update();
    app
}

fn eval(app: &App, chunk: &str) -> f64 {
    app.world()
        .non_send_resource::<UiScript>()
        .eval::<f64>(chunk)
        .unwrap()
}

/// The window-px rect of the quad tinted `rgb`, y-down.
fn quad(app: &App, rgb: [f32; 3]) -> Rect {
    let quads = &app.world().resource::<UiQuads>().quads;
    let q: &UiQuad = quads
        .iter()
        .find(|q| q.color[..3] == rgb)
        .expect("the marker quad extracted");
    q.rect
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

fn rect_close(got: Rect, want: Rect) -> bool {
    close(got.min.x, want.min.x)
        && close(got.min.y, want.min.y)
        && close(got.max.x, want.max.x)
        && close(got.max.y, want.max.y)
}

/// `UIParent:GetScale()` and `GetEffectiveScale()` answer the UI scale: it is `UIParent`'s own
/// scale, set through the worker Lua's `SetScale` reaches (`0x494550` → `0x76ac10`).
#[test]
fn uiparent_answers_the_ui_scale() {
    let app = app(1024, 768, 0.9);
    assert!(close(eval(&app, "return UIParent:GetScale()") as f32, 0.9));
    assert!(close(
        eval(&app, "return UIParent:GetEffectiveScale()") as f32,
        0.9
    ));
    assert!(close(
        eval(&app, "return Inside:GetEffectiveScale()") as f32,
        0.9
    ));
    assert_eq!(eval(&app, "return Outside:GetEffectiveScale()"), 1.0);
}

/// `GetScreenWidth`/`GetScreenHeight` (`0x48b480`/`0x48b4d0`) divide the 768-tall root by
/// `UIParent`'s scale (`0x494590`), so they answer in `UIParent`'s units.
#[test]
fn the_screen_size_answers_in_uiparent_units() {
    let app = app(1024, 768, 0.9);
    assert!(close(
        eval(&app, "return GetScreenHeight()") as f32,
        768.0 / 0.9
    ));
    assert!(close(
        eval(&app, "return GetScreenWidth()") as f32,
        1024.0 / 0.9
    ));
}

/// A frame with no parent hangs off the root, which keeps scale 1: one unit is `windowH/768` px
/// whatever the dial says.
#[test]
fn a_parentless_frame_is_not_scaled_by_the_ui_scale() {
    let app = app(1280, 1024, 0.9);
    let s = 1024.0 / 768.0;
    let got = quad(&app, [0.0, 1.0, 0.0]);
    assert!(
        rect_close(got, Rect::new(0.0, 1024.0 - 100.0 * s, 100.0 * s, 1024.0)),
        "got {got:?}"
    );
}

/// Inside `UIParent` nothing moves: the dial scales the subtree exactly as it scaled the seam.
#[test]
fn a_frame_inside_uiparent_lands_where_the_dial_put_it() {
    let app = app(1280, 1024, 0.9);
    let s = 1024.0 / 768.0 * 0.9;
    let got = quad(&app, [1.0, 0.0, 0.0]);
    assert!(
        rect_close(got, Rect::new(10.0 * s, 10.0 * s, 110.0 * s, 110.0 * s)),
        "got {got:?}"
    );
}

/// Whether `frame` is `UIParent` or hangs below it.
fn under_ui_parent(s: &UiScript, frame: benilla_ui::widget::FrameHandle) -> bool {
    let mut at = Some(frame);
    while let Some(h) = at {
        if s.frame_name(h).as_deref() == Some("UIParent") {
            return true;
        }
        at = s.frame_parent(h);
    }
    false
}

/// The whole stock interface, every frame shown: scaling `UIParent` by `S` in the 768-tall root
/// must place every quad of its subtree, and every hit, exactly where enlarging the root by `1/S`
/// with `UIParent` at scale 1 placed them, the seam having been the old home of the UI scale. A
/// frame-local length the engine forgets to scale (a border, an inset, a thumb) shows up here.
#[test]
fn the_stock_interface_inside_uiparent_is_unmoved_by_where_the_scale_lives() {
    let _data = benilla_formats::wow_data_or_skip!();
    const S: f32 = 0.9;
    let (w, h) = (768.0 * 16.0 / 9.0, 768.0);
    let mut script = UiScript::new().expect("VM");
    script.set_screen_size(w, h);
    let failures = super::load_default_ui(&script);
    assert!(failures.is_empty(), "default UI failed to load: {failures:?}");
    // Every frame shown, its parents first, so the most geometry the stock files declare draws.
    script
        .run(
            r#"
            local f = EnumerateFrames()
            while f do
                pcall(f.Show, f)
                f = EnumerateFrames(f)
            end
            "#,
        )
        .unwrap();
    let _ = script.take_errors();

    // The old home: a root `1/S` larger, `UIParent` at scale 1.
    script.set_screen_size(w / S, h / S);
    script.run("UIParent:SetScale(1)").unwrap();
    script.resolve();
    let old = script.extract();
    let grid: Vec<(f32, f32)> = (0..=40)
        .flat_map(|i| (0..=24).map(move |j| (w * i as f32 / 40.0, h * j as f32 / 24.0)))
        .collect();
    let old_hits: Vec<_> = grid
        .iter()
        .map(|&(x, y)| script.hit_test_frame(x / S, y / S))
        .collect();

    // The reference's: a 768-tall root, `UIParent` at scale `S`.
    script.set_screen_size(w, h);
    assert!(script.set_ui_scale(S));
    script.resolve();
    let new = script.extract();
    let new_hits: Vec<_> = grid
        .iter()
        .map(|&(x, y)| script.hit_test_frame(x, y))
        .collect();

    assert_eq!(old.len(), new.len(), "the same quads draw");
    let close = |a: f32, b: f32| (a - b).abs() < 0.02;
    let mut moved = Vec::new();
    for (a, b) in old.iter().zip(&new) {
        assert_eq!(a.target, b.target, "the same paint order");
        let Some(frame) = script.target_frame(b.target) else {
            continue;
        };
        if !under_ui_parent(&script, frame) {
            continue;
        }
        let same_rect = match (a.rect, b.rect) {
            (Some(ra), Some(rb)) => {
                close(ra.left * S, rb.left)
                    && close(ra.right * S, rb.right)
                    && close(ra.top * S, rb.top)
                    && close(ra.bottom * S, rb.bottom)
            }
            (None, None) => true,
            _ => false,
        };
        if !same_rect || !close(a.scale * S, b.scale) {
            moved.push(format!(
                "{:?} {:?}: {:?}×{S} vs {:?}",
                script.target_owner_name(b.target),
                std::mem::discriminant(&b.content),
                a.rect,
                b.rect
            ));
        }
    }
    assert!(
        moved.is_empty(),
        "{} quads inside UIParent moved, first: {:#?}",
        moved.len(),
        &moved[..moved.len().min(20)]
    );

    let inside = |hit: Option<benilla_ui::widget::FrameHandle>| {
        hit.is_none_or(|h| under_ui_parent(&script, h))
    };
    let mut rehit = Vec::new();
    for ((&at, a), b) in grid.iter().zip(&old_hits).zip(&new_hits) {
        if inside(*a) && inside(*b) && a != b {
            rehit.push(format!(
                "{at:?}: {:?} vs {:?}",
                a.and_then(|h| script.frame_name(h)),
                b.and_then(|h| script.frame_name(h))
            ));
        }
    }
    assert!(
        rehit.is_empty(),
        "{} hits inside UIParent changed, first: {:#?}",
        rehit.len(),
        &rehit[..rehit.len().min(20)]
    );
}
