//! The world hover's plate rebuilt inside the packets. `GameTooltip` remembers the unit it shows
//! (`+0x368`, stored at `0x52a070`), and `RefreshTooltipForUnit` (`0x4919d0`) builds it again
//! over that guid from the unit's callbacks: `NotifyUnitNameUpdate` (`0x491a00`) from the name and
//! creature answers (`0x5fb875`, `0x6046d1`) and the entry and `PLAYER_BYTES_3` watchers, ahead of
//! its `UNIT_NAME_UPDATE`, and the unit-flags watcher on the skinnable bit (`0x5ff7be`). So the
//! rebuild runs at the callback's packet, between its neighbours' events, with no anchor script
//! and no `UPDATE_MOUSEOVER_UNIT`, which are the publisher's alone.

use benilla_protocol::SessionEvent;
use benilla_ui::script::UiScript;
use bevy::prelude::*;

use super::unit_view_tests::{client, frame, health, values};

/// The target [`client`] seats: a creature.
const WOLF: u64 = 0xF130_0000_0044_0001;
/// The template entry the wolf's descriptor names.
const WOLF_ENTRY: u32 = 299;
/// `OBJECT_FIELD_ENTRY`.
const OBJECT_ENTRY: u16 = 3;
/// `UNIT_FLAG_SKINNABLE`.
const SKINNABLE: u32 = 0x0400_0000;

/// The wolf's answer packet.
fn wolf_named() -> SessionEvent {
    SessionEvent::CreatureName {
        entry: WOLF_ENTRY,
        name: Some("Wolf".into()),
        subname: None,
        creature_type: Some(1),
        pet_family: 0,
        rank: 0,
        type_flags: 0,
        pet_spell_list: 0,
        display_id: 0,
        civilian: false,
        racial_leader: false,
    }
}

/// The client with the wolf naming a template the cache lacks, and `GameTooltip` showing it as
/// `SetUnit("target")` builds it (`0x534a85` → `0x529fe0`). The spy notes each `UNIT_HEALTH` with
/// the plate's lines as its listener reads them, each `UNIT_NAME_UPDATE` likewise, each clear
/// (`OnTooltipCleared`, `0x530263`), each anchor script and each `UPDATE_MOUSEOVER_UNIT`.
fn plate_client() -> App {
    let mut app = client();
    let world = app.world_mut();
    let entity = world.resource::<crate::net::GuidIndex>().0[&WOLF];
    world
        .get_mut::<crate::net::ObjectStore>(entity)
        .expect("a store")
        .0
        .merge(benilla_protocol::ObjectFields::from_pairs(&[(
            OBJECT_ENTRY,
            WOLF_ENTRY,
        )]));
    crate::ui_script::test_ui::load_tooltip_template(world.non_send_resource::<UiScript>());
    crate::script_calls::with_hosted_vm(world, |s| {
        s.expect("a VM").run(
            r#"
            UNIT_SKINNABLE = "Skinnable"
            CreateFrame("GameTooltip", "GameTooltip", nil, "TestTooltipTemplate")
            function Plate()
                local t = {}
                for i = 1, GameTooltip:NumLines() do
                    t[i] = getglobal("GameTooltipTextLeft" .. i):GetText()
                end
                return table.concat(t, " | ")
            end
            function Look(event, unit) return Plate() end
            GameTooltip:SetScript("OnTooltipCleared", function() table.insert(Seen, "cleared") end)
            GameTooltip:SetScript("OnTooltipSetDefaultAnchor", function()
                table.insert(Seen, "anchor")
                GameTooltip:SetOwner(UIParent, "ANCHOR_NONE")
            end)
            local f = CreateFrame("Frame")
            f:RegisterEvent("UNIT_NAME_UPDATE")
            f:RegisterEvent("UPDATE_MOUSEOVER_UNIT")
            f:SetScript("OnEvent", function()
                table.insert(Seen, event .. ":" .. tostring(arg1) .. ":" .. Plate())
            end)
            GameTooltip:SetOwner(CreateFrame("Frame"), "ANCHOR_NONE")
            GameTooltip:SetUnit("target")
            Seen = {}
            "#,
        )
    })
    .expect("the plate shows the wolf");
    app
}

/// The creature answer rebuilds the plate at its packet (`0x6046d1` → `0x491a27` → `0x4919f2`),
/// ahead of its `UNIT_NAME_UPDATE`, whose listener reads the new name, and between the health
/// packets around it; nothing re-anchors and no `UPDATE_MOUSEOVER_UNIT` fires.
#[test]
fn a_creature_answer_rebuilds_the_plate_at_its_packet() {
    let mut app = plate_client();
    assert_eq!(
        frame(
            &mut app,
            vec![
                values(WOLF, &health(70)),
                wolf_named(),
                values(WOLF, &health(40)),
            ]
        ),
        [
            "UNIT_HEALTH:target:Unknown Being",
            "cleared",
            "UNIT_NAME_UPDATE:target:Wolf",
            "UNIT_HEALTH:target:Wolf",
        ]
    );
}

/// The unit-flags watcher rebuilds on the skinnable bit alone (`0x5ff7a4`-`0x5ff7be`), with no
/// event: the `UNIT_SKINNABLE` line (`0x52a914`) is there for the next packet's listener.
#[test]
fn the_skinnable_bit_rebuilds_the_plate_at_its_packet() {
    let mut app = plate_client();
    let flags = benilla_protocol::field::FIELD_UNIT_FLAGS;
    assert_eq!(
        frame(
            &mut app,
            vec![
                values(WOLF, &health(70)),
                values(WOLF, &[(flags, SKINNABLE)]),
                values(WOLF, &health(40)),
            ]
        ),
        [
            "UNIT_HEALTH:target:Unknown Being",
            "cleared",
            "UNIT_HEALTH:target:Unknown Being | Skinnable",
        ]
    );
}

/// A plate that no longer shows the unit is not built again: a hide clears the remembered unit
/// (`0x530a6b` → `0x52ffe0` → `0x530050`, `0x5300ad`), which `0x4919dc` compares. A negative
/// control: the frame-time rebuild never reached a hidden plate either, so it passes without the
/// packet rebuilds and guards only against their overreach.
#[test]
fn a_hidden_plate_is_not_rebuilt() {
    let mut app = plate_client();
    crate::script_calls::with_hosted_vm(app.world_mut(), |s| {
        s.expect("a VM").run("GameTooltip:Hide() Seen = {}")
    })
    .expect("hides");
    assert_eq!(
        frame(&mut app, vec![wolf_named()]),
        ["UNIT_NAME_UPDATE:target:"]
    );
}

/// A dead player's corpse, its owner [`OWNER`].
const CORPSE: u64 = 0xF000_0000_0000_0091;
/// The corpse's owner, whose name the cache lacks.
const OWNER: u64 = 0x91;
/// A locked door whose lock opens with the key [`KEY`].
const DOOR: u64 = 0xF110_0000_0000_0092;
const DOOR_ENTRY: u32 = 5000;
const LOCK: u32 = 77;
/// Key to Searing Gorge, which the item cache lacks.
const KEY: u32 = 5396;

/// `guid` created as `kind` with `pairs`.
fn object(
    guid: u64,
    kind: benilla_protocol::EntityKind,
    as_type: benilla_protocol::messages::ObjectType,
    pairs: &[(u16, u32)],
) -> SessionEvent {
    SessionEvent::ObjectCreate {
        guid,
        kind,
        display_id: None,
        position: [0.0; 3],
        orientation: 0.0,
        scale: 1.0,
        speeds: None,
        mover: None,
        transport_progress: None,
        transport: None,
        spline: None,
        fields: benilla_protocol::ObjectFields::from_pairs(pairs).into_created(as_type),
    }
}

/// [`plate_client`] with the corpse and the locked door streamed, the door's record and lock
/// known, a `UIParent`, and the plates' strings.
fn world_client() -> App {
    use benilla_protocol::messages::ObjectType;
    use benilla_protocol::EntityKind;
    let mut app = plate_client();
    let mut data = [0i32; 24];
    data[1] = LOCK as i32;
    app.world_mut()
        .resource_mut::<crate::go_templates::GameObjectTemplates>()
        .insert(DOOR_ENTRY, 0, "Gate".into(), &data);
    let mut slots = [benilla_formats::LockSlot::default(); benilla_formats::MAX_LOCK_SLOTS];
    slots[0] = benilla_formats::LockSlot {
        key_type: benilla_formats::LOCK_KEY_ITEM,
        index: KEY,
        skill: 0,
        action: 1,
    };
    app.world_mut().insert_resource(crate::go_templates::Locks(
        benilla_formats::LockCatalog::from_rows([(LOCK, slots)]),
    ));
    frame(
        &mut app,
        vec![
            object(
                CORPSE,
                EntityKind::Corpse,
                ObjectType::Corpse,
                &[(2, 0x81), (6, OWNER as u32), (7, 0)],
            ),
            object(
                DOOR,
                EntityKind::GameObject,
                ObjectType::GameObject,
                &[(2, 0x21), (3, DOOR_ENTRY), (9, 0x2), (14, 1), (21, 0)],
            ),
        ],
    );
    crate::script_calls::with_hosted_vm(app.world_mut(), |s| {
        s.expect("a VM").run(
            r#"
            CreateFrame("Frame", "UIParent")
            CORPSE_TOOLTIP = "Corpse of %s"
            LOCKED = "Locked"
            LOCKED_WITH_ITEM = "Requires %s"
            "#,
        )
    })
    .expect("seats");
    app
}

/// This frame's pick, then the hover's drive, as the WorldFrame's update runs (`0x481790`); the
/// spy's notes reset.
fn hover(app: &mut App, pick: crate::target::Hovered, object: crate::target::HoveredObject) {
    use bevy::ecs::system::RunSystemOnce;
    let world = app.world_mut();
    *world.resource_mut::<crate::target::Hovered>() = pick;
    *world.resource_mut::<crate::target::HoveredObject>() = object;
    world
        .run_system_once(crate::ui_tooltip::drive_mouseover_tooltip)
        .expect("the drive runs");
    world
        .non_send_resource::<UiScript>()
        .run("Seen = {}")
        .unwrap();
}

/// The corpse plate publishes at the hover with no line while the owner's name is not cached
/// (`0x52af7e`); the name's answer builds it again at its packet through the callback the lookup
/// registered (`0x52afe0` → `0x52aef0`), between the packets around it.
#[test]
fn a_corpse_owners_name_rebuilds_the_plate_at_its_packet() {
    let mut app = world_client();
    let corpse = app.world().resource::<crate::net::GuidIndex>().0[&CORPSE];
    hover(
        &mut app,
        crate::target::Hovered {
            corpse: Some(corpse),
            corpse_guid: Some(CORPSE),
            distance: 2.0,
            ..Default::default()
        },
        crate::target::HoveredObject::default(),
    );
    let named = SessionEvent::PlayerName {
        guid: OWNER,
        name: "Bob".into(),
        race: 1,
        class: 1,
        gender: 0,
    };
    assert_eq!(
        frame(
            &mut app,
            vec![values(WOLF, &health(70)), named, values(WOLF, &health(40))]
        ),
        [
            "UNIT_HEALTH:target:",
            "cleared",
            "UNIT_HEALTH:target:Corpse of Bob",
        ]
    );
    // The drive under the same hover builds nothing more (`0x4820b5`).
    hover(
        &mut app,
        crate::target::Hovered {
            corpse: Some(corpse),
            corpse_guid: Some(CORPSE),
            distance: 2.0,
            ..Default::default()
        },
        crate::target::HoveredObject::default(),
    );
    assert_eq!(
        crate::script_calls::eval_in_vm::<String>(app.world(), "return Plate()"),
        "Corpse of Bob"
    );
}

/// The door's plate leaves its key line out while the key's record is not cached (`0x52acab`);
/// the record's answer builds it again at its packet through the callback the lookup registered
/// (`0x52ad10` → `0x52aa20`), between the packets around it.
#[test]
fn a_lock_keys_record_rebuilds_the_plate_at_its_packet() {
    let mut app = world_client();
    let door = app.world().resource::<crate::net::GuidIndex>().0[&DOOR];
    hover(
        &mut app,
        crate::target::Hovered::default(),
        crate::target::HoveredObject {
            target: Some(door),
            guid: Some(DOOR),
            distance: 2.0,
        },
    );
    let key = SessionEvent::ItemTemplate {
        entry: KEY,
        info: Some(Box::new(crate::items::test_template(
            "Key to Searing Gorge",
        ))),
    };
    assert_eq!(
        frame(
            &mut app,
            vec![values(WOLF, &health(70)), key, values(WOLF, &health(40))]
        ),
        [
            "UNIT_HEALTH:target:Gate | Locked",
            "cleared",
            "UNIT_HEALTH:target:Gate | Locked | Requires Key to Searing Gorge",
        ]
    );
}

/// A locked door whose record the cache lacks.
const BARE_DOOR: u64 = 0xF110_0000_0000_0093;
const BARE_ENTRY: u32 = 5001;

/// A locked object whose record has not landed still shows the "Locked" line, which reads the
/// descriptor alone (`GAMEOBJECT_FLAGS` bit 1, `0x52aadc`-`0x52aaea`): no name line (`0x5f8100`
/// gives `""`, `0x5302a9`), and green (`0x52ab3d`), since the resolver finds no record
/// (`0x5f83d0` → `0x5f8418`). The record's answer builds nothing under the hover: its callback
/// (`0x5f78a0` → `0x5f7fb0`) reaches only the type handler.
#[test]
fn a_record_less_locked_object_shows_its_locked_line() {
    use benilla_protocol::messages::ObjectType;
    use benilla_protocol::EntityKind;
    let mut app = world_client();
    frame(
        &mut app,
        vec![object(
            BARE_DOOR,
            EntityKind::GameObject,
            ObjectType::GameObject,
            &[(2, 0x21), (3, BARE_ENTRY), (9, 0x2), (14, 1), (21, 0)],
        )],
    );
    let door = app.world().resource::<crate::net::GuidIndex>().0[&BARE_DOOR];
    hover(
        &mut app,
        crate::target::Hovered::default(),
        crate::target::HoveredObject {
            target: Some(door),
            guid: Some(BARE_DOOR),
            distance: 2.0,
        },
    );
    let look = r#"
        local r, g, b = GameTooltipTextLeft1:GetTextColor()
        local function byte(c) return math.floor(c * 255 + 0.5) end
        return Plate() .. " " .. byte(r) .. " " .. byte(g) .. " " .. byte(b)
    "#;
    assert_eq!(
        crate::script_calls::eval_in_vm::<String>(app.world(), look),
        "Locked 64 192 64"
    );
    let record = SessionEvent::GameObjectInfo {
        entry: BARE_ENTRY,
        type_id: 0,
        display_id: 0,
        name: "Gate".into(),
        data: [0; 24],
    };
    assert_eq!(frame(&mut app, vec![record]), Vec::<String>::new());
    assert_eq!(
        app.world()
            .resource::<crate::go_templates::GameObjectTemplates>()
            .name(BARE_ENTRY),
        Some("Gate")
    );
    assert_eq!(
        crate::script_calls::eval_in_vm::<String>(app.world(), "return Plate()"),
        "Locked"
    );
}
