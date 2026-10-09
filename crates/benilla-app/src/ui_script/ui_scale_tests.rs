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
