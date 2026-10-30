//! The nameplates inside the packets. The frame's gate binds and seats them (`0x60f600` from each
//! unit's update, `0x607ef9`), but what a bound plate shows of its unit moves at the unit's
//! callbacks, inside the packet that moved it: the bar's watchers (`0x7cc570`, linked at the bind
//! by `0x7cc510`), the health watcher's colour and flash (`0x60471b`, `0x604764`), a log line's
//! flash (`0x62abac`), the level watcher (`0x60ce5b`), the raid board's icons (`0x608a90`) and
//! the gate `InvalidateNameDesc` runs (`0x60706a`), which a name answer's release (`0x6046c1`)
//! hands a new bind. So a listener a later packet of the frame fires reads its packet's plate.

use benilla_protocol::field::{FIELD_UNIT_BYTES_1, FIELD_UNIT_LEVEL};
use benilla_protocol::SessionEvent;
use benilla_ui::script::UiScript;
use bevy::prelude::*;

use super::unit_view_tests::{client, frame, health, values};

/// The target [`client`] seats.
const WOLF: u64 = 0xF130_0000_0044_0001;
/// The unit beside it, 50 of 100 health, which wears the plate.
const BOAR: u64 = 0xF130_0000_0045_0002;
/// `OBJECT_FIELD_ENTRY`.
const OBJECT_ENTRY: u16 = 3;
/// The template the boar's descriptor names, which the cache lacks.
const BOAR_ENTRY: u32 = 1984;

/// [`client`] with both kinds of plate on and the boar's plate bound, as the frame's gate bound
/// it (health 30 of 40, a red bar, "unit 0x…", level 12), its spatial tests passed; `Plate()` and
/// `Bar()` name its frame and its bar. The spy's `Look` is the test's.
fn plated_client(look: &str) -> App {
    let mut app = client();
    let world = app.world_mut();
    world.insert_resource(crate::vplates::VPlateMode {
        enemies: true,
        friends: true,
    });
    world
        .resource_mut::<crate::vplates::PlateSpots>()
        .0
        .insert(BOAR);
    let mut script = world.non_send_resource_mut::<UiScript>();
    crate::vplates::plates_for_test(&mut script, &[BOAR]);
    script
        .run(&format!(
            r#"
            function Plate() return WorldFrame:GetChildren() end
            function Bar() return Plate():GetChildren() end
            function Name() local _, _, name = Plate():GetRegions() return name end
            function Shown(f) if f:IsShown() then return "shown" end return "hidden" end
            function Byte(c) return math.floor(c * 255 + 0.5) end
            function Look(event, unit) {look} end
            Seen = {{}}
            "#
        ))
        .unwrap();
    app
}

/// The bar's watchers run in the notify pass of the packet that moved the unit's health
/// (`0x7cc570`): the range, re-firing the held value (`0x7cc5a8`), then the value (`0x7cc5be`),
/// each with its `OnValueChanged`. A listener a later packet fires reads that packet's value.
#[test]
fn a_plates_bar_moves_inside_the_packet_that_moves_its_units_health() {
    let mut app = plated_client("return Bar():GetValue()");
    crate::script_calls::eval_in_vm::<()>(
        app.world(),
        r#"Bar():SetScript("OnValueChanged", function() table.insert(Seen, "bar:" .. arg1) end)"#,
    );
    assert_eq!(
        frame(
            &mut app,
            vec![
                values(BOAR, &health(25)),
                values(WOLF, &health(70)),
                values(BOAR, &health(10)),
            ]
        ),
        [
            "bar:30",
            "bar:25",
            "UNIT_HEALTH:target:25",
            "bar:25",
            "bar:10"
        ]
    );
}

/// The health watcher's plate legs run at its packet, ahead of the bridge (`0x6046f0`): the bar's
/// colour from the reaction (`0x60471b` → `0x607080` → `0x7cbaa0`), the faction tables' neutral
/// here, yellow over the bind's red, then the flash on a loss (`0x604764`), the name red.
#[test]
fn the_health_watcher_recolours_and_flashes_the_plate_at_its_packet() {
    let mut app = plated_client(
        r#"local r, g, b = Bar():GetStatusBarColor()
        local _, ng = Name():GetTextColor()
        return Byte(r) .. "," .. Byte(g) .. "," .. Byte(b) .. "/" .. Byte(ng)"#,
    );
    assert_eq!(
        frame(
            &mut app,
            vec![
                values(WOLF, &health(70)),
                values(BOAR, &health(25)),
                values(WOLF, &health(60))
            ]
        ),
        [
            "UNIT_HEALTH:target:255,0,0/255",
            "UNIT_HEALTH:target:255,255,0/0"
        ]
    );
}

/// A log line's victim flashes at its packet, ahead of the line (`0x62aac0` at `0x62abac`): an
/// environmental hit on the boar, then the target's health.
#[test]
fn a_logged_hit_flashes_the_victims_plate_at_its_packet() {
    let mut app = plated_client(r#"local _, g = Name():GetTextColor() return Byte(g)"#);
    let fall =
        SessionEvent::EnvironmentalDamageLog(benilla_protocol::messages::EnvironmentalDamageLog {
            victim: BOAR,
            damage_type: 2,
            damage: 5,
            absorb: 0,
            resist: 0,
        });
    assert_eq!(
        frame(
            &mut app,
            vec![values(WOLF, &health(70)), fall, values(WOLF, &health(60))]
        ),
        ["UNIT_HEALTH:target:255", "UNIT_HEALTH:target:0"]
    );
}

/// The level watcher runs the plate's level at its packet (`0x6045b0` → `0x60cdd0` at
/// `0x60ce5b` → `0x7cbb40`).
#[test]
fn the_level_watcher_writes_the_plates_level_at_its_packet() {
    let mut app =
        plated_client(r#"local _, _, _, level = Plate():GetRegions() return level:GetText()"#);
    assert_eq!(
        frame(
            &mut app,
            vec![
                values(WOLF, &health(70)),
                values(BOAR, &[(FIELD_UNIT_LEVEL, 20)]),
                values(WOLF, &health(60)),
            ]
        ),
        ["UNIT_HEALTH:target:12", "UNIT_HEALTH:target:20"]
    );
}

/// The creature answer releases the unit's plate (`0x604660` at `0x6046c1`), then its name update
/// runs the gate (`0x6046d1` → `0x491a2e` → `0x60706a`), which binds a plate again with the
/// answer's name, all at the answer's packet: the hide, the show, then the next packet's listener
/// reads the name.
#[test]
fn a_creature_answer_binds_the_plate_again_with_its_name_at_its_packet() {
    let mut app = plated_client(r#"return Shown(Plate()) .. ":" .. Name():GetText()"#);
    let world = app.world_mut();
    let boar = world.resource::<crate::net::GuidIndex>().0[&BOAR];
    world
        .get_mut::<crate::net::ObjectStore>(boar)
        .expect("a store")
        .0
        .merge(benilla_protocol::ObjectFields::from_pairs(&[(
            OBJECT_ENTRY,
            BOAR_ENTRY,
        )]));
    crate::script_calls::eval_in_vm::<()>(
        app.world(),
        r#"
        Plate():SetScript("OnHide", function() table.insert(Seen, "hide") end)
        Plate():SetScript("OnShow", function() table.insert(Seen, "show") end)
        "#,
    );
    let named = SessionEvent::CreatureName {
        entry: BOAR_ENTRY,
        name: Some("Boar".into()),
        subname: None,
        creature_type: Some(1),
        pet_family: 0,
        rank: 0,
        type_flags: 0,
        pet_spell_list: 0,
        display_id: 0,
        civilian: false,
        racial_leader: false,
    };
    assert_eq!(
        frame(
            &mut app,
            vec![values(WOLF, &health(70)), named, values(WOLF, &health(60))]
        ),
        [
            "UNIT_HEALTH:target:shown:unit 0xf130000000450002",
            "hide",
            "show",
            "UNIT_HEALTH:target:shown:Boar",
        ]
    );
}

/// `UNIT_FIELD_BYTES_1` byte 3's watcher runs the gate on its bit 0x1 (`0x5ff846` → `0x60706a`):
/// a unit gone to the creep look has no plate (`0x60f62e`), released at the packet.
#[test]
fn a_creep_look_releases_the_plate_at_its_packet() {
    let mut app = plated_client("return Shown(Plate())");
    assert_eq!(
        frame(
            &mut app,
            vec![
                values(WOLF, &health(70)),
                values(BOAR, &[(FIELD_UNIT_BYTES_1, 0x0100_0000)]),
                values(WOLF, &health(60)),
            ]
        ),
        ["UNIT_HEALTH:target:shown", "UNIT_HEALTH:target:hidden"]
    );
}

/// A unit leaving the world frees its plate at its packet (`CGUnit_C::Release` at `0x5fbba9`).
#[test]
fn a_destroyed_units_plate_hides_at_its_packet() {
    let mut app = plated_client("return Shown(Plate())");
    assert_eq!(
        frame(
            &mut app,
            vec![
                values(WOLF, &health(70)),
                SessionEvent::ObjectDestroyed(BOAR),
                values(WOLF, &health(60)),
            ]
        ),
        ["UNIT_HEALTH:target:shown", "UNIT_HEALTH:target:hidden"]
    );
}

/// The raid target handler sets each moved unit's plate icon (`0x4ba28a` → `0x608a90`) ahead of
/// its `RAID_TARGET_UPDATE` (`0x4ba348`), whose listener reads the plate's icon.
#[test]
fn a_raid_mark_shows_on_the_plate_ahead_of_its_event() {
    let mut app = plated_client("return ''");
    crate::script_calls::eval_in_vm::<()>(
        app.world(),
        r#"
        local f = CreateFrame("Frame")
        f:RegisterEvent("RAID_TARGET_UPDATE")
        f:SetScript("OnEvent", function()
            local raid = ({ Plate():GetRegions() })[6]
            local left, top = raid:GetTexCoord()
            table.insert(Seen, event .. ":" .. Shown(raid) .. ":" .. left .. "," .. top)
        end)
        "#,
    );
    assert_eq!(
        frame(
            &mut app,
            vec![SessionEvent::RaidTargetSet {
                icon: 5,
                guid: BOAR
            }]
        ),
        ["RAID_TARGET_UPDATE:shown:0.25,0.25"]
    );
}
