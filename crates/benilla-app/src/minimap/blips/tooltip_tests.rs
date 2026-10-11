//! The blip tooltip driver against the UI's own hover: the Minimap's probe and leave.

use benilla_ui::script::UiScript;
use bevy::prelude::*;

use super::{drive_blip_tooltip, MinimapBlipHover};
use crate::names::NameCache;
use crate::net::NetCommands;

/// A full-screen `UIParent`, a mouse-enabled `Minimap` at (600, 400)–(740, 540) with the cursor on
/// it, and a `GameTooltip`; the driver runs each update.
fn minimap_app() -> App {
    let mut app = App::new();
    let (tx, _rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx))
        .init_resource::<NameCache>()
        .init_resource::<MinimapBlipHover>()
        .add_systems(Update, drive_blip_tooltip);
    let mut script = UiScript::new().unwrap();
    script.set_screen_size(800.0, 600.0);
    script
        .run(
            r#"
            UIParent = CreateFrame("Frame", "UIParent"); UIParent:SetAllPoints()
            local map = CreateFrame("Minimap", "Minimap", UIParent)
            map:SetPoint("BOTTOMLEFT", 600, 400); map:SetWidth(140); map:SetHeight(140)
            map:EnableMouse(true)
            CreateFrame("GameTooltip", "GameTooltip", UIParent):Hide()
            "#,
        )
        .unwrap();
    script.resolve();
    script.mouse_move(670.0, 470.0);
    app.insert_non_send_resource(script);
    app
}

fn blip(app: &mut App, hover: MinimapBlipHover) {
    *app.world_mut().resource_mut::<MinimapBlipHover>() = hover;
    app.update();
}

fn shown(app: &mut App) -> bool {
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .eval::<bool>("return GameTooltip:IsShown() == 1")
        .unwrap()
}

/// Off every blip, the probe `0x4eb0c0` hides `GameTooltip` directly (`0x4eb55f`–`0x4eb567`),
/// whoever holds it: an addon tooltip taken over the blip's goes too.
#[test]
fn a_blip_hover_loss_hides_whoever_holds_the_tooltip() {
    let mut app = minimap_app();
    blip(&mut app, MinimapBlipHover::Landmark("Stormwind".into()));
    assert!(shown(&mut app), "the blip tooltip is up");
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(r#"GameTooltip:SetOwner(UIParent, "ANCHOR_NONE"); GameTooltip:SetText("Addon")"#)
        .unwrap();
    blip(&mut app, MinimapBlipHover::None);
    assert!(!shown(&mut app), "the addon's tooltip is hidden");
    assert!(app
        .world_mut()
        .non_send_resource_mut::<UiScript>()
        .take_errors()
        .is_empty());
}
