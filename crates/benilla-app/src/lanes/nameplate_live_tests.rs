//! The nameplates inside the packets. The frame's gate binds and seats them (`0x60f600` from each
//! unit's update, `0x607ef9`), but what a bound plate shows of its unit moves at the unit's
//! callbacks, inside the packet that moved it: the bar's watchers (`0x7cc570`, linked at the bind
//! by `0x7cc510`), the health watcher's colour and flash (`0x60471b`, `0x604764`), a log line's
//! flash (`0x62abac`), the level watcher (`0x60ce5b`), the raid board's icons (`0x608a90`) and
//! the gate `InvalidateNameDesc` runs (`0x60706a`), which a name answer's release (`0x6046c1`)
//! hands a new bind. So a listener a later packet of the frame fires reads its packet's plate.

use benilla_protocol::field::{
    FIELD_UNIT_BYTES_1, FIELD_UNIT_FLAGS, FIELD_UNIT_HEALTH, FIELD_UNIT_LEVEL, FIELD_UNIT_MAXHEALTH,
};
use benilla_protocol::{EntityKind, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;

use super::unit_view_tests::{client, created, frame, health, values};

/// The client's own guid.
const ME: u64 = 0x7;
/// `PLAYER_BYTES_3`, whose byte 1 is the drunkenness.
const PLAYER_BYTES_3: u16 = 195;
/// The drunkenness 50 in `PLAYER_BYTES_3`: a cut of `trunc(50 * 0.05 + 0.5)`, 3 levels.
const DRUNK_50: u32 = 50 << 8;
/// `UNIT_FLAG_PLAYER_CONTROLLED`, which takes `CanAttack`'s player arm: with no faction tables we
/// may attack the boar.
const PLAYER_CONTROLLED: u32 = 0x8;

/// The target [`client`] seats.
const WOLF: u64 = 0xF130_0000_0044_0001;
/// The unit beside it, 50 of 100 health, which wears the plate.
const BOAR: u64 = 0xF130_0000_0045_0002;
/// `OBJECT_FIELD_ENTRY`.
const OBJECT_ENTRY: u16 = 3;
/// The template the boar's descriptor names, which the cache lacks.
const BOAR_ENTRY: u32 = 1984;

/// [`client`] with both kinds of plate on and the boar's plate bound, as the frame's gate bound
/// it (health 30 of 40, a red bar, "unit 0x…", level 12), its spatial tests passed
/// ([`helpers`]). The spy's `Look` is the test's.
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
    helpers(&script, look);
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

/// The script helpers both clients define: `Plate()`, `Bar()`, `Name()` and `Level()` name the
/// first plate and its parts.
fn helpers(script: &UiScript, look: &str) {
    script
        .run(&format!(
            r#"
            function Plate() return WorldFrame:GetChildren() end
            function Bar() return Plate():GetChildren() end
            function Name() local _, _, name = Plate():GetRegions() return name end
            function Level() local _, _, _, level = Plate():GetRegions() return level end
            function Shown(f) if f:IsShown() then return "shown" end return "hidden" end
            function Byte(c) return math.floor(c * 255 + 0.5) end
            function Look(event, unit) {look} end
            Seen = {{}}
            "#
        ))
        .unwrap();
}

/// The bar's watcher loads both fields signed and clamps neither (`fild` at `0x7cc599`,
/// `0x7cc5b3`, into `0x783270`): a maximum of 0 is a range of 0 to 0, and `0xffffffff` is -1.
#[test]
fn a_plates_bar_reads_its_units_health_signed_and_unclamped() {
    let mut app = plated_client("local _, max = Bar():GetMinMaxValues() return max");
    assert_eq!(
        frame(
            &mut app,
            vec![
                values(BOAR, &[(FIELD_UNIT_MAXHEALTH, 0)]),
                values(WOLF, &health(70)),
                values(BOAR, &[(FIELD_UNIT_MAXHEALTH, 0xFFFF_FFFF)]),
                values(WOLF, &health(60)),
            ]
        ),
        ["UNIT_HEALTH:target:0", "UNIT_HEALTH:target:-1"]
    );
}

/// A drunk player sees a unit it can attack lower on its plate (`0x7cbb7a` → `0x7cbce0`): at 50,
/// 20 less `trunc(50 * 0.05 + 0.5)`, 17, where `UnitLevel`'s `50 * 5 * 0.01` (`0x5180c8`) cuts 2;
/// at 10, 30 less 1, where `UnitLevel` cuts none.
#[test]
fn a_drunk_player_reads_a_lower_level_on_the_plate_of_a_unit_it_can_attack() {
    let mut app = plated_client("return Level():GetText()");
    assert_eq!(
        frame(
            &mut app,
            vec![
                values(
                    ME,
                    &[
                        (PLAYER_BYTES_3, DRUNK_50),
                        (FIELD_UNIT_FLAGS, PLAYER_CONTROLLED)
                    ]
                ),
                values(BOAR, &[(FIELD_UNIT_LEVEL, 20)]),
                values(WOLF, &health(70)),
                values(ME, &[(PLAYER_BYTES_3, 10 << 8)]),
                values(BOAR, &[(FIELD_UNIT_LEVEL, 30)]),
                values(WOLF, &health(60)),
            ]
        ),
        ["UNIT_HEALTH:target:17", "UNIT_HEALTH:target:29"]
    );
}

/// `InvalidateNameDesc`'s gate on a unit that keeps its plate refreshes its level while the latch
/// is set (`0x60706a` → `0x6086e0`, `0x60875f` → `0x7cbb40`): our level moves to the boar's,
/// setting the latch (`0x5edb93`), and the boar's name update in the same frame (`0x604651` →
/// `0x491a2e`) recolours its level yellow at once, where the frame's gate would only after the
/// packets.
#[test]
fn a_packets_gate_on_a_kept_plate_refreshes_its_level_on_the_latch() {
    let mut app = plated_client(
        r#"local r, g, b = Level():GetTextColor()
        return Byte(r) .. "," .. Byte(g) .. "," .. Byte(b)"#,
    );
    record(&mut app, BOAR_ENTRY, "Boar");
    assert_eq!(
        frame(
            &mut app,
            vec![
                values(BOAR, &[(FIELD_UNIT_LEVEL, 20)]),
                values(WOLF, &health(70))
            ]
        ),
        ["UNIT_HEALTH:target:255,25,25"],
        "a level 1 player sees level 20 red"
    );
    assert_eq!(
        frame(
            &mut app,
            vec![
                values(ME, &[(FIELD_UNIT_LEVEL, 20)]),
                values(BOAR, &[(OBJECT_ENTRY, BOAR_ENTRY)]),
                values(WOLF, &health(60)),
            ]
        ),
        ["UNIT_HEALTH:target:255,255,0"]
    );
}

/// A creature record cached for `entry`.
fn record(app: &mut App, entry: u32, name: &str) {
    app.world_mut()
        .resource_mut::<crate::names::NameCache>()
        .insert_creature(
            entry,
            Some(crate::names::CreatureRecord {
                name: name.into(),
                subname: None,
                creature_type: 1,
                pet_family: 0,
                rank: 0,
                type_flags: 0,
                pet_spell_list: 0,
                civilian: false,
                racial_leader: false,
                display_id: 0,
            }),
        );
}

/// `OnGuildRecordForName` (`0x5dd860`): the record the name block's guild line missed lands, and
/// the callback runs `InvalidateNameDesc` on the unit (`0x5dd88b`), whose gate refuses it now,
/// gone unselectable (`0x60f622`), and releases its plate at the answer's packet.
#[test]
fn a_guild_record_the_name_block_waited_on_gates_the_plate_at_its_packet() {
    let mut app = plated_client("return Shown(Plate())");
    let world = app.world_mut();
    let commands = crate::net::NetCommands(world.resource::<crate::net::NetCommands>().0.clone());
    let guild = world.resource::<crate::ui_guild::GuildState>();
    let fields = benilla_protocol::ObjectFields::from_pairs(&[(GUILD_ID, 7)]);
    assert_eq!(
        crate::ui_guild::unit_guild_name(BOAR, &fields, guild, &commands),
        None,
        "the line misses"
    );
    let answer = SessionEvent::GuildQueryResponse(benilla_protocol::messages::GuildQueryResponse {
        guild_id: 7,
        name: "Legacy".into(),
        ..Default::default()
    });
    assert_eq!(
        frame(
            &mut app,
            vec![
                values(WOLF, &health(70)),
                values(BOAR, &[(FIELD_UNIT_FLAGS, 1 << 25)]),
                answer,
                values(WOLF, &health(60)),
            ]
        ),
        ["UNIT_HEALTH:target:shown", "UNIT_HEALTH:target:hidden"]
    );
}

/// `PLAYER_GUILDID`.
const GUILD_ID: u16 = 191;

/// The two health watchers on one unit run in the order they were linked onto its list
/// (`0x467f00`, `0x467f65`): the boar's plate was bound before it was targeted, whose token flag
/// linked its unit-field watchers then (`0x493540` → `0x60c4a0` → `0x51bbb0`), so the bar moves
/// first and the `UNIT_HEALTH("target")` listener reads the packet's value on it. The wolf was
/// the target before its plate was bound, its bridge ahead of the bar.
#[test]
fn a_units_bar_and_bridge_run_in_the_order_they_were_linked() {
    let mut app = plated_client("return Bar():GetValue()");
    {
        let world = app.world_mut();
        world.resource_mut::<crate::target::Selection>().last = Some(BOAR);
        crate::script_calls::with_hosted_vm(world, |s| {
            s.expect("a VM").run("TargetLastTarget()").expect("runs");
        });
    }
    let _ = frame(&mut app, vec![]);
    assert_eq!(
        frame(&mut app, vec![values(BOAR, &health(25))]),
        ["UNIT_HEALTH:target:25"]
    );
}

/// [`client`] with the frame's gate able to run (`drive_vplates`): both kinds of plate on, the
/// camera looking down on us, and the boar alone drawn, so the gate gives it the one plate. The
/// spy's `Look` is the test's; the frame's gate has run once.
fn framed_client(look: &str) -> App {
    use bevy::camera::visibility::SetViewVisibility;
    let mut app = client();
    let world = app.world_mut();
    world.insert_resource(crate::vplates::VPlateMode {
        enemies: true,
        friends: true,
    });
    world.init_resource::<crate::player::CameraControl>();
    let mut camera = Camera::default();
    camera.computed.clip_from_view =
        Mat4::perspective_infinite_reverse_rh(1.2, 1024.0 / 768.0, 0.1);
    camera.computed.target_info = Some(bevy::camera::RenderTargetInfo {
        physical_size: UVec2::new(1024, 768),
        scale_factor: 1.0,
    });
    world.spawn((
        camera,
        Transform::from_xyz(0.0, 60.0, 0.0).looking_at(Vec3::ZERO, Vec3::NEG_Z),
        benilla_world::view::WorldCamera,
    ));
    let index = world.resource::<crate::net::GuidIndex>().0.clone();
    world.entity_mut(index[&ME]).insert(crate::net::Embodied);
    let part = world
        .spawn((
            Mesh3d::default(),
            ViewVisibility::HIDDEN,
            ChildOf(index[&BOAR]),
        ))
        .id();
    world.get_mut::<ViewVisibility>(part).unwrap().set_visible();
    let mut script = world.non_send_resource_mut::<UiScript>();
    script.set_screen_size(1024.0, 768.0);
    script
        .run(r#"WorldFrame = CreateFrame("WorldFrame", "WorldFrame") WorldFrame:SetAllPoints()"#)
        .unwrap();
    script.resolve();
    helpers(&script, look);
    crate::vplates::drive_vplates_once(world);
    app
}

/// The frame's gate binds the boar's plate.
#[test]
fn the_framed_client_binds_the_boars_plate() {
    let app = framed_client("return ''");
    let shown: String = crate::script_calls::eval_in_vm(
        app.world(),
        "return Shown(Plate()) .. ':' .. Bar():GetValue()",
    );
    assert_eq!(shown, "shown:50");
}

/// The drunkenness's watcher on the active player sets the level latch (`0x5e2a90` at
/// `0x5e2b42` → `0x605550`), so the frame's gate after the packets refreshes the kept plate's
/// level (`0x60875f`), which the drunkenness lowers (`0x7cbce0`).
#[test]
fn a_change_of_our_drunkenness_refreshes_the_kept_plates_level_at_the_frame() {
    let mut app = framed_client("return ''");
    let _ = frame(&mut app, vec![values(BOAR, &[(FIELD_UNIT_LEVEL, 20)])]);
    let level = |app: &App| -> String {
        crate::script_calls::eval_in_vm(app.world(), "return Level():GetText()")
    };
    assert_eq!(level(&app), "20");
    let _ = frame(
        &mut app,
        vec![values(
            ME,
            &[
                (PLAYER_BYTES_3, DRUNK_50),
                (FIELD_UNIT_FLAGS, PLAYER_CONTROLLED),
            ],
        )],
    );
    crate::vplates::drive_vplates_once(app.world_mut());
    assert_eq!(level(&app), "17");
}

/// The spatial tests run with both kinds of plate off (`0x60f773` projects where the gate runs):
/// a script turns them on in a frame whose gate has not run yet, and a packet's gate in it binds
/// the boar, which the frame before left in view.
#[test]
fn a_packets_gate_binds_a_plate_in_the_frame_the_plates_come_on() {
    let mut app = framed_client("return Shown(Plate())");
    record(&mut app, BOAR_ENTRY, "Boar");
    let set_mode = |app: &mut App, on: bool| {
        app.world_mut().insert_resource(crate::vplates::VPlateMode {
            enemies: on,
            friends: on,
        });
    };
    set_mode(&mut app, false);
    crate::vplates::drive_vplates_once(app.world_mut());
    set_mode(&mut app, true);
    assert_eq!(
        frame(
            &mut app,
            vec![
                values(WOLF, &health(70)),
                values(BOAR, &[(OBJECT_ENTRY, BOAR_ENTRY)]),
                values(WOLF, &health(60)),
            ]
        ),
        ["UNIT_HEALTH:target:hidden", "UNIT_HEALTH:target:shown"]
    );
}

/// The frame's gate after the packets moves nothing a packet's callback already moved: the bar's
/// value, its colour and the level stay as the callbacks left them, and no `OnValueChanged` runs
/// again (`0x6086e0` on a kept plate takes `0x60875f`'s branch, not the bind's). A guard against a
/// double write, not a test of a fix: the driver before the callbacks moved in passes it too.
#[test]
fn the_frames_gate_after_the_packets_writes_no_kept_plate_again() {
    let mut app = framed_client("return ''");
    crate::script_calls::eval_in_vm::<()>(
        app.world(),
        r#"Bar():SetScript("OnValueChanged", function() table.insert(Seen, "bar:" .. arg1) end)"#,
    );
    let read = r#"local r, g, b = Bar():GetStatusBarColor()
        return Bar():GetValue() .. "/" .. Byte(r) .. "," .. Byte(g) .. "," .. Byte(b) .. "/"
            .. (Level():GetText() or "")"#;
    let seen = frame(
        &mut app,
        vec![
            values(BOAR, &[(FIELD_UNIT_LEVEL, 12)]),
            values(BOAR, &[(FIELD_UNIT_HEALTH, 20)]),
        ],
    );
    assert_eq!(
        seen,
        ["bar:50", "bar:20"],
        "the bar's watcher at its packet"
    );
    let before: String = crate::script_calls::eval_in_vm(app.world(), read);
    crate::vplates::drive_vplates_once(app.world_mut());
    let after: String = crate::script_calls::eval_in_vm(app.world(), read);
    let again: Vec<String> = crate::script_calls::eval_in_vm(app.world(), "return Seen");
    assert_eq!(after, before);
    assert!(again.is_empty(), "the frame fired the bar again: {again:?}");
}

/// The party's slot writers refresh a held member's plate colour (`0x607080`): the seat's once
/// the slot holds it (`0x4e83dd`), an enemy-faction player turning friendly (`0x6062b0`), its bar
/// blue; the clear's ahead of the slot's zeroing (`0x4e8568`, `0x4e858c`), which reads the member
/// in the party still, so the bar stays blue until a later refresh, its health watcher's.
#[test]
fn the_party_seat_and_clear_recolour_the_members_plate() {
    use crate::target::relations::fixture::{factions, HUMAN, ORC, PC};
    use benilla_protocol::messages::{member_status, GroupMemberEntry};
    const MATE: u64 = 0x22;
    const TEMPLATE: u16 = 35;
    let mut app = client();
    app.insert_resource(factions());
    let _ = frame(
        &mut app,
        vec![
            values(ME, &[(TEMPLATE, HUMAN), (FIELD_UNIT_FLAGS, PC)]),
            created(
                MATE,
                EntityKind::Player,
                &[
                    (TEMPLATE, ORC),
                    (FIELD_UNIT_FLAGS, PC),
                    (FIELD_UNIT_HEALTH, 100),
                    (FIELD_UNIT_MAXHEALTH, 100),
                ],
            ),
        ],
    );
    let world = app.world_mut();
    world.insert_resource(crate::vplates::VPlateMode {
        enemies: true,
        friends: true,
    });
    world
        .resource_mut::<crate::vplates::PlateSpots>()
        .0
        .insert(MATE);
    let mut script = world.non_send_resource_mut::<UiScript>();
    crate::vplates::plates_for_test(&mut script, &[MATE]);
    helpers(
        &script,
        r#"local r, g, b = Bar():GetStatusBarColor()
        return Byte(r) .. "," .. Byte(g) .. "," .. Byte(b)"#,
    );
    let list = SessionEvent::GroupList {
        group_type: 0,
        own_flags: 0,
        members: vec![GroupMemberEntry {
            name: String::new(),
            guid: MATE,
            status: member_status::ONLINE,
            flags: 0,
        }],
        leader: ME,
        loot: None,
    };
    assert_eq!(
        frame(&mut app, vec![list, values(WOLF, &health(70))]),
        ["UNIT_HEALTH:target:0,0,255"],
        "seated"
    );
    // Painted red behind the plate's back: only the clear's own refresh paints it blue again.
    crate::script_calls::eval_in_vm::<()>(app.world(), "Bar():SetStatusBarColor(1, 0, 0)");
    assert_eq!(
        frame(
            &mut app,
            vec![SessionEvent::GroupDestroyed, values(WOLF, &health(60))]
        ),
        ["UNIT_HEALTH:target:0,0,255"],
        "cleared while still in its slot"
    );
    assert_eq!(
        frame(
            &mut app,
            vec![
                values(MATE, &[(FIELD_UNIT_HEALTH, 90)]),
                values(WOLF, &health(50))
            ]
        ),
        ["UNIT_HEALTH:target:255,0,0"],
        "the health watcher's, out of the party"
    );
}

/// A party that becomes a raid: the list's emptier takes a party-only member's pet's flag back to
/// 0 (`0x4e8530` → `0x60c50c`), the seat links its unit-field watchers again at the tail
/// (`0x4e83d6`), and the raid rows the same list rebuilds after it (`0x5e6eb4`) add to flags
/// already set (`0x4bab65`). So the pet's plate, bound between the two lists, now runs ahead of its
/// bridge, and each `UNIT_HEALTH` listener reads the packet's value on the bar.
#[test]
fn a_party_turned_raid_links_a_members_pets_watchers_behind_its_plate() {
    use benilla_protocol::messages::{member_status, GroupMemberEntry};
    const MATE: u64 = 0x22;
    const PET: u64 = 0xF140_0000_0046_0003;
    let list = |group_type| SessionEvent::GroupList {
        group_type,
        own_flags: 0,
        members: vec![GroupMemberEntry {
            name: String::new(),
            guid: MATE,
            status: member_status::ONLINE,
            flags: 0,
        }],
        leader: ME,
        loot: None,
    };
    let mut app = client();
    let _ = frame(
        &mut app,
        vec![
            created(
                PET,
                EntityKind::Unit,
                &[(FIELD_UNIT_HEALTH, 100), (FIELD_UNIT_MAXHEALTH, 100)],
            ),
            created(
                MATE,
                EntityKind::Player,
                &[
                    (FIELD_UNIT_HEALTH, 100),
                    (FIELD_UNIT_MAXHEALTH, 100),
                    (SUMMON, PET as u32),
                    (SUMMON + 1, (PET >> 32) as u32),
                ],
            ),
            list(0),
        ],
    );
    let world = app.world_mut();
    world
        .resource_mut::<crate::vplates::PlateSpots>()
        .0
        .insert(PET);
    let mut script = world.non_send_resource_mut::<UiScript>();
    // The tokens as the frame's push leaves them for the bind, which runs with no host lent here.
    script.set_unit_guids(&benilla_ui::script::UnitGuids {
        player: ME,
        target: WOLF,
        party: [MATE, 0, 0, 0],
        party_pets: [PET, 0, 0, 0],
        held: [(ME, 0), (WOLF, 0), (BOAR, 0), (MATE, 0), (PET, 0)]
            .into_iter()
            .collect(),
        ..Default::default()
    });
    crate::vplates::plates_for_test(&mut script, &[PET]);
    helpers(&script, "return Bar():GetValue()");
    let _ = frame(&mut app, vec![list(crate::ui_party::GROUPTYPE_RAID)]);
    let seen = frame(&mut app, vec![values(PET, &[(FIELD_UNIT_HEALTH, 25)])]);
    assert!(
        !seen.is_empty() && seen.iter().all(|s| s.ends_with(":25")),
        "{seen:?}"
    );
}

/// `UNIT_FIELD_SUMMON`, a unit's summoned pet.
const SUMMON: u16 = 8;

/// `UNIT_FIELD_CHARM`, the active player's charm.
const CHARM: u16 = 6;

/// Our charm on the boar broken and cast again inside one frame's packets: `OnCharmOrSummonChanged`
/// reads the boar's token flags again each time (`0x5e2702`, `0x5e2743` → `0x60c520`), which
/// unlink and then link its unit-field watchers at the tail (`0x60c679`, `0x60c65c`), behind the
/// plate bound while the first charm held. The `UNIT_HEALTH("pet")` listener reads the packet's
/// value on the bar.
#[test]
fn a_charm_cast_again_links_the_pets_watchers_behind_its_plate() {
    let mut app = plated_client("return Bar():GetValue()");
    let charm = |guid: u64| {
        values(
            ME,
            &[(CHARM, guid as u32), (CHARM + 1, (guid >> 32) as u32)],
        )
    };
    let _ = frame(&mut app, vec![charm(BOAR)]);
    {
        let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
        // The tokens as the frame's push leaves them for the bind, which runs with no host lent
        // here.
        script.set_unit_guids(&benilla_ui::script::UnitGuids {
            player: ME,
            pet: BOAR,
            target: WOLF,
            held: [(ME, 0), (WOLF, 0), (BOAR, 0)].into_iter().collect(),
            ..Default::default()
        });
        crate::vplates::sync_plates_for_test(&mut script, &[]);
        crate::vplates::sync_plates_for_test(&mut script, &[BOAR]);
    }
    let _ = frame(&mut app, vec![charm(0), charm(BOAR)]);
    assert_eq!(
        frame(&mut app, vec![values(BOAR, &health(25))]),
        ["UNIT_HEALTH:pet:25"]
    );
}
