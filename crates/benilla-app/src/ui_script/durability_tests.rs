//! The durability figure: stock `DurabilityFrame.lua` over the engine's `GetInventoryAlertStatus`.

use super::test_ui::load_ui as load_xml;
use benilla_ui::script::{
    InvSlotView, InventorySlots, ItemTemplateView, QuadContent, ScriptValue, UiScript,
};

fn harness() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in [
        "Interface\\FrameXML\\Fonts.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        r"Interface\FrameXML\UIParent.xml",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\BasicControls.xml",
        "Interface\\FrameXML\\LocaleProperties.lua",
        "Interface\\FrameXML\\StaticPopup.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        "Interface\\FrameXML\\UIDropDownMenu.xml",
        "Interface\\FrameXML\\BattlefieldFrame.xml",
        "Interface\\FrameXML\\Minimap.xml",
        "Interface\\FrameXML\\DurabilityFrame.xml",
    ] {
        load_xml(&s, f);
    }
    // Deviation: the seat refreshes on every alert recompute, because the stock one goes stale
    // when a side glyph appears while shown and hangs the shield off the screen edge. The app
    // installs this after every load.
    super::manifest::install_durability_reseat(&s).unwrap();
    s
}

fn slot(item_id: u32, durability: Option<(u32, u32)>) -> Option<InvSlotView> {
    Some(InvSlotView {
        item_id,
        durability,
        count: 1,
        quality: 2,
        ..Default::default()
    })
}

/// The painted colour of the shown quad that samples this `<TexCoords>` cell. The weapon and
/// off-weapon glyphs share a cell but never show together.
fn cell_color(s: &mut UiScript, cell: [f32; 4]) -> Option<[f32; 4]> {
    s.resolve();
    s.extract().iter().find_map(|q| match &q.content {
        QuadContent::Texture {
            tex_coords: Some(tc),
            color,
            ..
        } if tc
            .edges()
            .iter()
            .zip(cell)
            .all(|(a, b)| (a - b).abs() < 1e-4) =>
        {
            Some(color.unwrap_or([1.0, 1.0, 1.0, 1.0]))
        }
        _ => None,
    })
}

const HEAD_CELL: [f32; 4] = [0.0, 0.140625, 0.0, 0.171875];
const LEGS_CELL: [f32; 4] = [0.46875, 0.6875, 0.171875, 0.3203125];
const WEAPON_CELL: [f32; 4] = [0.0, 0.140625, 0.3203125, 0.6640625];
const SHIELD_CELL: [f32; 4] = [0.1875, 0.375, 0.3203125, 0.5546875];

const RED: [f32; 4] = [0.93, 0.07, 0.07, 1.0];
const YELLOW: [f32; 4] = [1.0, 0.82, 0.18, 1.0];

/// `DurabilityFrame_SetAlerts`: one body alert shows every body piece, the rest faded white.
#[test]
fn armor_guy_shows_red_broken_yellow_damaged_and_hides_clean() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    // The frame is authored shown; `PLAYER_ENTERING_WORLD` runs `SetAlerts`, which hides it.
    s.fire_event("PLAYER_ENTERING_WORLD", vec![ScriptValue::Str("".into())]);
    assert!(
        !s.eval::<bool>("return DurabilityFrame:IsShown()").unwrap(),
        "clean gear → no armor guy"
    );

    let mut inv: InventorySlots = Default::default();
    inv[16] = slot(25, Some((0, 20)));
    s.set_inventory_slots(inv);
    assert!(s.errors().is_empty(), "alert errors: {:?}", s.errors());
    assert!(
        s.eval::<bool>("return DurabilityFrame:IsShown() and DurabilityWeapon:IsShown()")
            .unwrap(),
        "a broken weapon shows the frame + its glyph"
    );
    assert!(
        !s.eval::<bool>("return DurabilityHead:IsShown()").unwrap(),
        "no body alert → the body stays hidden"
    );
    assert_eq!(cell_color(&mut s, WEAPON_CELL), Some(RED), "broken → red");

    // Legs at 3 points: damaged is an absolute 1..=5 points left (`0x4c8012`).
    let mut inv: InventorySlots = Default::default();
    inv[7] = slot(39, Some((3, 25)));
    s.set_inventory_slots(inv);
    assert!(
        s.eval::<bool>(
            "return DurabilityFrame:IsShown() and DurabilityLegs:IsShown() and DurabilityHead:IsShown()"
        )
        .unwrap(),
        "one body alert shows the whole body"
    );
    assert!(
        !s.eval::<bool>("return DurabilityWeapon:IsShown()").unwrap(),
        "the repaired weapon glyph hides"
    );
    assert_eq!(
        cell_color(&mut s, LEGS_CELL),
        Some(YELLOW),
        "damaged → yellow"
    );
    assert_eq!(
        cell_color(&mut s, HEAD_CELL),
        Some([1.0, 1.0, 1.0, 0.5]),
        "un-alerted body piece rides faded white"
    );

    // Absolute, not a ratio: 5 of 100 is damaged, 6 of 20 is not.
    let mut inv: InventorySlots = Default::default();
    inv[7] = slot(39, Some((5, 100)));
    s.set_inventory_slots(inv);
    assert!(
        s.eval::<bool>("return DurabilityFrame:IsShown()").unwrap(),
        "5 points left is damaged at ANY max (absolute law)"
    );
    let mut inv: InventorySlots = Default::default();
    inv[7] = slot(39, Some((6, 20)));
    s.set_inventory_slots(inv);
    assert!(
        !s.eval::<bool>("return DurabilityFrame:IsShown()").unwrap(),
        "6 points left is never damaged (absolute law, no percentage)"
    );
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// Item flag `0x10` forces red and `0x08` (wrapped) silences the region (`0x4c7faa`, `0x4c7fc8`);
/// region 12 is low ammo (20 or fewer carried reads 3), which the 1.12 FrameXML never reads.
#[test]
fn flag_bits_and_the_low_ammo_region() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.fire_event("PLAYER_ENTERING_WORLD", vec![ScriptValue::Str("".into())]);

    let mut inv: InventorySlots = Default::default();
    let mut v = slot(25, Some((20, 20))).unwrap();
    v.flags = 0x10;
    inv[16] = Some(v);
    s.set_inventory_slots(inv);
    assert_eq!(
        s.eval::<i64>("return GetInventoryAlertStatus(9)").unwrap(),
        4,
        "force-red bit → broken at full durability"
    );
    assert_eq!(cell_color(&mut s, WEAPON_CELL), Some(RED));

    let mut inv: InventorySlots = Default::default();
    let mut v = slot(25, Some((0, 20))).unwrap();
    v.flags = 0x08;
    inv[16] = Some(v);
    s.set_inventory_slots(inv);
    assert!(
        !s.eval::<bool>("return DurabilityFrame:IsShown()").unwrap(),
        "a wrapped item never alerts"
    );

    let mut inv: InventorySlots = Default::default();
    inv[0] = slot(2512, None).map(|mut v| {
        v.count = 15;
        v
    });
    s.set_inventory_slots(inv);
    assert_eq!(
        s.eval::<i64>("return GetInventoryAlertStatus(12)").unwrap(),
        3,
        "low carried ammo reads 3 on the 12th region"
    );
    assert!(
        !s.eval::<bool>("return DurabilityFrame:IsShown()").unwrap(),
        "the 1.12 armor guy never shows for ammo"
    );
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// `SetAlerts`' Shield arm swaps to the off-weapon glyph when `OffhandHasWeapon()` (item class 2).
#[test]
fn off_hand_glyph_follows_what_the_hand_holds() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.set_item_template(
        2362,
        ItemTemplateView {
            name: "Worn Wooden Shield".into(),
            class: 4, // armor
            ..Default::default()
        },
    );
    s.set_item_template(
        2488,
        ItemTemplateView {
            name: "Worn Dagger".into(),
            class: 2, // weapon
            ..Default::default()
        },
    );

    let mut inv: InventorySlots = Default::default();
    inv[17] = slot(2362, Some((0, 20)));
    s.set_inventory_slots(inv);
    assert!(
        s.eval::<bool>("return DurabilityShield:IsShown() and not DurabilityOffWeapon:IsShown()")
            .unwrap(),
        "a shield lights the shield glyph"
    );
    assert_eq!(cell_color(&mut s, SHIELD_CELL), Some(RED));

    let mut inv: InventorySlots = Default::default();
    inv[17] = slot(2488, Some((0, 16)));
    s.set_inventory_slots(inv);
    assert!(
        s.eval::<bool>("return DurabilityOffWeapon:IsShown() and not DurabilityShield:IsShown()")
            .unwrap(),
        "an off-hand weapon swaps to the off-weapon glyph"
    );
    assert_eq!(
        cell_color(&mut s, WEAPON_CELL),
        Some(RED),
        "the weapon cell paints the off-weapon red"
    );
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// The manage pass seats `TOPRIGHT` at the cluster's `BOTTOMRIGHT` less `CONTAINER_OFFSET_X`, and
/// 20 more while the shield, off-weapon or ranged glyph shows (`UIParent.lua:1759`).
#[test]
fn manage_pass_seats_the_frame_inside_the_cluster_edge() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.fire_event("PLAYER_ENTERING_WORLD", vec![ScriptValue::Str("".into())]);

    // Broken ranged; `OnShow` runs the pass. The main-hand glyph hangs off the body's left, so
    // the offset leaves it out.
    let mut inv: InventorySlots = Default::default();
    inv[18] = slot(2504, Some((0, 20)));
    s.set_inventory_slots(inv);
    s.resolve();
    let delta: f32 = s
        .eval("return MinimapCluster:GetRight() - DurabilityFrame:GetRight()")
        .unwrap();
    assert_eq!(
        delta, 20.0,
        "a right-side glyph seats the frame 20 further left"
    );

    s.set_inventory_slots(Default::default());
    let mut inv: InventorySlots = Default::default();
    inv[7] = slot(39, Some((3, 25)));
    s.set_inventory_slots(inv);
    s.resolve();
    let delta: f32 = s
        .eval("return MinimapCluster:GetRight() - DurabilityFrame:GetRight()")
        .unwrap();
    assert_eq!(
        delta, 0.0,
        "body-only alerts seat flush with the cluster edge"
    );
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// Gear streams in slot by slot at login, so a side glyph can appear while the frame is shown;
/// the re-seat deviation still pulls it 20 in so the shield's ~17-unit overhang clears the edge.
#[test]
fn a_late_side_glyph_refreshes_the_seat_while_the_frame_stays_shown() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.set_item_template(
        25,
        ItemTemplateView {
            name: "Worn Shortsword".into(),
            class: 2, // weapon
            ..Default::default()
        },
    );
    s.set_item_template(
        30,
        ItemTemplateView {
            name: "Large Round Shield".into(),
            class: 4, // armor (a shield, not an off-hand weapon)
            ..Default::default()
        },
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![ScriptValue::Str("".into())]);

    let mut inv: InventorySlots = Default::default();
    inv[16] = slot(25, Some((0, 20)));
    s.set_inventory_slots(inv);
    s.resolve();
    assert!(
        s.eval::<bool>("return DurabilityFrame:IsShown() and not DurabilityShield:IsShown()")
            .unwrap(),
        "weapon-only: frame shown, no shield glyph yet"
    );
    let delta: f32 = s
        .eval("return MinimapCluster:GetRight() - DurabilityFrame:GetRight()")
        .unwrap();
    assert_eq!(delta, 0.0, "the left-side weapon glyph needs no extra room");

    // The frame is already shown, so no show transition fires.
    let mut inv: InventorySlots = Default::default();
    inv[16] = slot(25, Some((0, 20)));
    inv[17] = slot(30, Some((3, 35)));
    s.set_inventory_slots(inv);
    s.resolve();
    assert!(
        s.eval::<bool>("return DurabilityShield:IsShown()").unwrap(),
        "the off-hand's arrival shows the shield glyph"
    );
    let delta: f32 = s
        .eval("return MinimapCluster:GetRight() - DurabilityFrame:GetRight()")
        .unwrap();
    assert_eq!(
        delta, 20.0,
        "a side glyph arriving while shown must still pull the frame 20 in"
    );
    let overhang: f32 = s
        .eval("return DurabilityShield:GetRight() - GetScreenWidth()")
        .unwrap();
    assert!(
        overhang <= 0.5,
        "shield glyph clips off the right edge (overhang {overhang} px)"
    );
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// `QuestWatchFrame` is last in the right-side walk, seated at the running `anchorY` after the
/// durability frame's height (`UIParent.lua:1770`).
#[test]
fn the_quest_tracker_stacks_below_the_durability_guy() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in [
        "Interface\\FrameXML\\Fonts.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        r"Interface\FrameXML\UIParent.xml",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        "Interface\\FrameXML\\BasicControls.xml",
        "Interface\\FrameXML\\LocaleProperties.lua",
        "Interface\\FrameXML\\StaticPopup.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        "Interface\\FrameXML\\UIDropDownMenu.xml",
        "Interface\\FrameXML\\BattlefieldFrame.xml",
        "Interface\\FrameXML\\Minimap.xml",
        "ScrollTemplates.xml",
        "Interface\\FrameXML\\DurabilityFrame.xml",
        "Interface\\FrameXML\\BasicControls.xml",
        "Interface\\FrameXML\\ItemButtonTemplate.xml",
        "Interface\\FrameXML\\QuestFrame.xml",
        r"Interface\FrameXML\MainMenuBarMicroButtons.xml",
        "Interface\\FrameXML\\QuestLogFrame.xml",
    ] {
        load_xml(&s, f);
    }
    super::manifest::install_durability_reseat(&s).unwrap();

    let mut inv: InventorySlots = Default::default();
    inv[18] = slot(2504, Some((0, 20)));
    s.set_inventory_slots(inv);
    s.resolve();

    let dur_bottom: f32 = s.eval("return DurabilityFrame:GetBottom()").unwrap();
    let watch_top: f32 = s.eval("return QuestWatchFrame:GetTop()").unwrap();
    assert!(
        (watch_top - dur_bottom).abs() <= 0.5,
        "tracker top {watch_top} must sit flush under the durability bottom {dur_bottom}"
    );
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}
