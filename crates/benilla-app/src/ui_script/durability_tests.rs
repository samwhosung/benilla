//! The durability alert ("armor guy") against the reference's own `DurabilityFrame.xml`/`.lua`,
//! executed off the player's chain since 1751's fourth window:
//! the engine's `GetInventoryAlertStatus` statuses (recomputed on every inventory push, a change
//! firing `UPDATE_INVENTORY_ALERTS`) drive the ref's own SetAlerts law — body pieces show
//! together when any body region alerts, showSeparate pieces (Weapon/Shield/Ranged) each show
//! only themselves, the shield glyph swaps for the off-weapon glyph when the off hand holds a
//! WEAPON, and the whole frame hides at zero alerts.

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
        // The reference's own file. This module carried a private disk-only `load_xml` until the
        // swap, which structurally could not name a chain entry — [`super::test_ui::load_ui`] is
        // the one reader that speaks both stores, and it is why that helper exists.
        "Interface\\FrameXML\\DurabilityFrame.xml",
    ] {
        load_xml(&s, f);
    }
    // The app installs this at the end of every real load; a test VM has to say so itself. It is
    // benilla's stated repair of a reference bug — the seat going stale when a side glyph appears
    // while the frame is already shown — and its whole reasoning is at the constant.
    super::manifest::install_durability_reseat(&s).unwrap();
    s
}

/// One equipped slot view with a live durability pair.
fn slot(item_id: u32, durability: Option<(u32, u32)>) -> Option<InvSlotView> {
    Some(InvSlotView {
        item_id,
        durability,
        count: 1,
        quality: 2,
        ..Default::default()
    })
}

/// The atlas cell (`<TexCoords>`) of a shown region's quad → its painted vertex color; `None`
/// when no shown quad samples that cell. The weapon and off-weapon glyphs share one cell — in
/// any single state at most one of the pair is shown, so the cell stays unambiguous.
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

/// The show/hide + color law across the three states: clean (hidden), broken main hand (the
/// weapon glyph alone, red), damaged legs (ALL body pieces show, legs yellow, the rest faded
/// white), then repaired (hidden again).
#[test]
fn armor_guy_shows_red_broken_yellow_damaged_and_hides_clean() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    // The ref's initial settle: the frame is authored shown; PLAYER_ENTERING_WORLD's SetAlerts
    // hides it while everything is clean.
    s.fire_event("PLAYER_ENTERING_WORLD", vec![ScriptValue::Str("".into())]);
    assert!(
        !s.eval::<bool>("return DurabilityFrame:IsShown()").unwrap(),
        "clean gear → no armor guy"
    );

    // Broken main hand: the weapon glyph alone, painted the ref's red.
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

    // Damaged legs (3 points left — the byte law's ABSOLUTE 1..=5, `cmp [D+0xa0],5` at
    // `0x4c8012`): every body piece shows, legs yellow, the un-alerted head the faded
    // white(0.5); the weapon glyph (repaired now) hides.
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

    // The threshold is ABSOLUTE, not a ratio: 5 points on a 100-max piece (5%) is damaged;
    // 6 points on a 20-max piece (30%... and even 6/100) is not — `1..=5` exactly.
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

/// The FLAGS bits (`0x4c7faa`, `0x4c7fc8`): `0x10` forces red regardless of durability;
/// `0x08` (wrapped gift) silences the region entirely. And the client's 12th region — low ammo
/// (carried count <= 20 → 3) — answers through `GetInventoryAlertStatus(12)` even though the
/// 1.12 FrameXML never reads it.
#[test]
fn flag_bits_and_the_low_ammo_region() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.fire_event("PLAYER_ENTERING_WORLD", vec![ScriptValue::Str("".into())]);

    // Force-red: full durability, bit 0x10 → status 4, the glyph paints red.
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

    // Wrapped: broken durability but bit 0x08 → silent.
    let mut inv: InventorySlots = Default::default();
    let mut v = slot(25, Some((0, 20))).unwrap();
    v.flags = 0x08;
    inv[16] = Some(v);
    s.set_inventory_slots(inv);
    assert!(
        !s.eval::<bool>("return DurabilityFrame:IsShown()").unwrap(),
        "a wrapped item never alerts"
    );

    // Low ammo: 15 carried → region 12 reads 3; the armor guy stays hidden (FrameXML's 1..=11).
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

/// The off-hand switch (ref SetAlerts' Shield arm): a broken SHIELD lights the shield glyph; a
/// broken off-hand WEAPON (template class 2) swaps it for the off-weapon glyph.
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

    // Broken shield → the shield glyph, red; the off-weapon glyph stays hidden.
    let mut inv: InventorySlots = Default::default();
    inv[17] = slot(2362, Some((0, 20)));
    s.set_inventory_slots(inv);
    assert!(
        s.eval::<bool>("return DurabilityShield:IsShown() and not DurabilityOffWeapon:IsShown()")
            .unwrap(),
        "a shield lights the shield glyph"
    );
    assert_eq!(cell_color(&mut s, SHIELD_CELL), Some(RED));

    // Broken off-hand weapon → the glyphs swap.
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

/// The manage pass owns the seat (ref UIParent.lua:1758-1768): the authored XML anchor (+40,
/// off-screen — director-caught) dies on the first pass; the frame's TOPRIGHT re-seats at the
/// cluster's BOTTOMRIGHT minus CONTAINER_OFFSET_X (0 with no right multibars), minus 20 more
/// while a side glyph (weapon/shield/ranged) extends the art rightward. OnShow/OnHide re-fire
/// the pass on every alert transition.
#[test]
fn manage_pass_seats_the_frame_inside_the_cluster_edge() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.fire_event("PLAYER_ENTERING_WORLD", vec![ScriptValue::Str("".into())]);

    // Broken RANGED: the show transition's OnShow runs the pass; the RIGHT-side glyph pulls
    // the frame 20 further left. (The ref's offset list deliberately excludes the main-hand
    // weapon glyph — it hangs off the LEFT of the body and needs no extra room.)
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

    // Repair (hide), then damaged legs (show; body only): flush with the cluster edge.
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

/// The seat stays fresh when a side glyph arrives AFTER the frame is already shown — the real
/// login order, equipment streaming in slot by slot with the weapon breaking before the
/// off-hand's slot lands (char "One": Worn Shortsword 0/20, then Large Round Shield 3/35). The
/// weapon glyph hangs off the body's LEFT and needs no room; once the shield glyph appears on the
/// RIGHT the frame must pull 20 further in so its ~17-unit overhang clears the screen edge.
/// Regression: the offset only refreshed on the frame's own show/hide transition, so a glyph that
/// appeared while the frame stayed shown left it stale and hung the shield off-screen
/// (director-caught). The OnEvent re-manage keeps it fresh on every alert recompute.
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

    // 1) The weapon breaks first: the frame shows with only the LEFT-side weapon glyph → flush.
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

    // 2) The off-hand's slot lands later — the frame is ALREADY shown, so no show transition
    //    fires. The shield glyph now extends the art right; the seat must still pull in by 20.
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
    // The director's symptom, directly: the shield glyph's right edge clears the screen.
    let overhang: f32 = s
        .eval("return DurabilityShield:GetRight() - GetScreenWidth()")
        .unwrap();
    assert!(
        overhang <= 0.5,
        "shield glyph clips off the right edge (overhang {overhang} px)"
    );
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// The quest tracker drops below the durability guy instead of overlapping his corner (ref
/// UIParent.lua:1770 — QuestWatchFrame is last in the right-side walk, seated at the running
/// anchorY after the durability height is subtracted). Both frames anchor the same
/// MinimapCluster BOTTOMRIGHT; benilla shipped only the durability arm and left the tracker
/// colliding there (director-caught, over "The Captain's Chest").
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

    // A right-side glyph shows the 65-tall durability frame under the minimap.
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
