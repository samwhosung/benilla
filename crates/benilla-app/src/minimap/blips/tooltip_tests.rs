//! The blip tooltip driver against the UI's own hover: the Minimap's probe and leave.

use benilla_ui::script::UiScript;
use bevy::prelude::*;

use super::{drive_blip_tooltip, BlipBlock, BlipName, MinimapBlipHover};
use crate::names::NameCache;
use crate::net::NetCommands;

/// A full-screen `UIParent`, a mouse-enabled `Minimap` at (600, 400)–(740, 540) with the cursor on
/// it, and a `GameTooltip`; the driver runs each update.
fn minimap_app() -> App {
    let mut app = App::new();
    let (tx, _rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx))
        .init_resource::<NameCache>()
        .init_resource::<crate::net::GuidIndex>()
        .init_resource::<MinimapBlipHover>()
        .add_systems(Update, drive_blip_tooltip);
    let mut script = UiScript::new().unwrap();
    crate::ui_script::test_ui::load_tooltip_template(&script);
    script.set_screen_size(800.0, 600.0);
    script
        .run(
            r#"
            UIParent = CreateFrame("Frame", "UIParent"); UIParent:SetAllPoints()
            local map = CreateFrame("Minimap", "Minimap", UIParent)
            map:SetPoint("BOTTOMLEFT", 600, 400); map:SetWidth(140); map:SetHeight(140)
            map:EnableMouse(true)
            CreateFrame("GameTooltip", "GameTooltip", UIParent, "TestTooltipTemplate"):Hide()
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

/// A landmark icon under the cursor.
fn landmark(name: &str) -> MinimapBlipHover {
    let mut hover = MinimapBlipHover::default();
    hover.push(BlipBlock::Icon, BlipName::Known(name.into()), false);
    hover
}

/// A tracked unit's dot under the cursor.
fn unit(guid: u64) -> MinimapBlipHover {
    let mut hover = MinimapBlipHover::default();
    hover.push(BlipBlock::Dot(1), BlipName::Unit(guid), false);
    hover
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
    blip(&mut app, landmark("Stormwind"));
    assert!(shown(&mut app), "the blip tooltip is up");
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(r#"GameTooltip:SetOwner(UIParent, "ANCHOR_NONE"); GameTooltip:SetText("Addon")"#)
        .unwrap();
    blip(&mut app, MinimapBlipHover::default());
    assert!(!shown(&mut app), "the addon's tooltip is hidden");
    assert!(app
        .world_mut()
        .non_send_resource_mut::<UiScript>()
        .take_errors()
        .is_empty());
}

/// The probe's name lookup passes no callback (`0x4eb33c`): a hovered unit the name cache does not
/// hold shows nothing and asks nothing, however long the hover.
#[test]
fn a_hovered_unnamed_blip_only_reads() {
    let mut app = minimap_app();
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx));
    for _ in 0..600 {
        blip(&mut app, unit(0x9));
    }
    assert!(!shown(&mut app), "no name, no tooltip");
    assert!(crate::query_cache::queries(&rx).is_empty());
}

/// A live unit names itself through `GetUnitName` with a callback (`0x4eb31a`, `0x5f1ed0`), which
/// every probe runs: one ask while the answer is pending, and the probe after a refusal asks again.
#[test]
fn a_hovered_live_unit_is_asked_by_each_probe_once_per_answer() {
    const UNIT: u64 = 0x9; // a player's guid
    let mut app = minimap_app();
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx));
    let e = app
        .world_mut()
        .spawn(crate::net::ObjectStore(
            benilla_protocol::ObjectFields::default()
                .into_created(benilla_protocol::messages::ObjectType::Player),
        ))
        .id();
    app.world_mut()
        .resource_mut::<crate::net::GuidIndex>()
        .0
        .insert(UNIT, e);
    blip(&mut app, unit(UNIT));
    assert_eq!(crate::query_cache::queries(&rx).len(), 1, "the probe asks");
    blip(&mut app, unit(UNIT));
    assert!(
        crate::query_cache::queries(&rx).is_empty(),
        "once while the answer is pending"
    );
    app.world_mut()
        .resource_mut::<NameCache>()
        .insert_player(UNIT, String::new(), None);
    blip(&mut app, unit(UNIT));
    assert_eq!(
        crate::query_cache::queries(&rx).len(),
        1,
        "the probe after the refusal asks again"
    );
}

/// What the probe sees with the cursor on the player's own position, the disc 133 yd across.
fn ctx_at_centre() -> super::BlipCtx {
    super::BlipCtx {
        center: Vec2::ZERO,
        side: super::BLIP_BASIS_PX,
        px_per_yd: super::BLIP_BASIS_PX * 0.5 / 133.0,
        radius_yd: 133.0,
        z: 0,
        alpha: 1.0,
        wx: 0.0,
        wy: 0.0,
        wz: 0.0,
        cursor: Some(Vec2::ZERO),
    }
}

/// `0x4eb0c0` keeps every landmark icon the cursor is over as a bit of `0xbdcbac`, and the build
/// joins a line for each in list order, `\n` between them (`0x4eb67a`-`0x4eb6ed`, `0x835144`),
/// into the one gold line it adds (`0x530380`): the gossip marker's static slot before the
/// corpse's, both stacked on the player.
#[test]
fn stacked_landmarks_each_get_a_line_in_list_order() {
    let marker = benilla_formats::AreaPoi {
        importance: 0,
        icon: 6,
        faction_id: 0,
        pos: [0.0; 3],
        continent_id: 0,
        flags: 2,
        area_id: 0,
        name: "Guards' directions".into(),
        description: String::new(),
        world_state_id: 0,
    };
    let corpse = super::corpse_landmark([0.0; 3], 0, "Corpse".into());
    let mut hover = MinimapBlipHover::default();
    let mut quads = crate::ui_pass::UiQuads::default();
    super::emit_landmarks(
        &ctx_at_centre(),
        None,
        Some(&marker),
        Some(&corpse),
        0,
        Some(&Handle::default()),
        &mut quads,
        &mut hover,
    );
    let mut app = minimap_app();
    blip(&mut app, hover);
    assert!(shown(&mut app));
    let text = app
        .world_mut()
        .non_send_resource_mut::<UiScript>()
        .eval::<String>("return GameTooltipTextLeft1:GetText()")
        .unwrap();
    assert_eq!(text, "Guards' directions\nCorpse");
}

/// The build's order (`0x4eb58c`-`0x4eb840`): the rim arrows, the party and pet arrow frames, the
/// icons, then the dot categories 0 to 4, whatever order the emitters found them in. A grey dot
/// is wrapped in `|cffb0b0b0`…`|r` (`0x4eb7cf`); a dot whose name the cache lacks has no line
/// (`0x4eb785`).
#[test]
fn the_lines_join_in_the_probes_order_greyed_and_named() {
    const MEMBER: u64 = 0x7;
    const UNNAMED: u64 = 0x8;
    let mut app = minimap_app();
    app.world_mut()
        .resource_mut::<NameCache>()
        .insert_player(MEMBER, "Aldwyn".into(), None);
    let mut hover = MinimapBlipHover::default();
    let known = |n: &str| BlipName::Known(n.into());
    hover.push(BlipBlock::Icon, known("Goldshire"), false);
    hover.push(BlipBlock::RimArrow, known("Stormwind"), false);
    hover.push(BlipBlock::Dot(0), known("Copper Vein"), true);
    hover.push(BlipBlock::Dot(1), BlipName::Unit(UNNAMED), false);
    hover.push(BlipBlock::Dot(3), known("Marshal"), false);
    hover.push(BlipBlock::GroupArrow, BlipName::Unit(MEMBER), false);
    blip(&mut app, hover);
    let text = app
        .world_mut()
        .non_send_resource_mut::<UiScript>()
        .eval::<String>("return GameTooltipTextLeft1:GetText()")
        .unwrap();
    assert_eq!(
        text,
        "Stormwind\nAldwyn\nGoldshire\n|cffb0b0b0Copper Vein|r\nMarshal"
    );
}
