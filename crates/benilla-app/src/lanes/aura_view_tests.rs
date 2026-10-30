//! What an aura listener reads, as its own packet left the auras. `UnitBuff` walks the unit's
//! fields at the call (`0x519500`), and the active player's aura watcher `0x4e4140` rebuilds the
//! buff cache and signals `PLAYER_AURAS_CHANGED` inside the notify pass of each packet that moves
//! the 0x138 bytes from `UNIT_FIELD_AURA` (`0x4e4100`: `0x4e4109`, `0x4e4115`; `0x4e4170`, the
//! signal at `0x4e437f`), so `GetPlayerBuff` reads the cache as that packet left it (`0x4e45d0`).

use benilla_protocol::SessionEvent;
use benilla_ui::script::UiScript;
use bevy::prelude::*;

use super::unit_view_tests::{client, values, wire_packets};

const ME: u64 = 0x7;
/// `UNIT_FIELD_FLAGS`, whose bit 27 opens `UnitBuff`'s unit gate (`0x5195b6`).
const FLAGS: u16 = 46;
/// `UNIT_FIELD_AURA`, `UNIT_FIELD_AURAFLAGS` and `UNIT_FIELD_AURAAPPLICATIONS`.
const AURA: u16 = 47;
const AURAFLAGS: u16 = 95;
const APPLICATIONS: u16 = 113;
/// A flag nibble with a shown bit (`& 0xe`) and the cancelable bit.
const SHOWN: u32 = 0x9;

/// One frame: the packets, each packet's lanes fired after its handlers, then the frame's drain;
/// what the aura spy saw.
fn frame(app: &mut App, packets: Vec<Vec<SessionEvent>>) -> Vec<String> {
    let world = app.world_mut();
    crate::net::handlers::dispatch(world, wire_packets(packets));
    crate::lanes::fire_in_order(world);
    let s = world.non_send_resource_mut::<UiScript>();
    let out = s.eval::<Vec<String>>("return Auras").unwrap();
    s.run("Auras = {}").unwrap();
    let errors = s.errors();
    assert!(errors.is_empty(), "script errors: {errors:?}");
    out
}

/// [`client`] with our buffs shown to `UnitBuff`, and a spy on `UNIT_AURA` that reads the first
/// buff's stack count and on `PLAYER_AURAS_CHANGED` that reads each cached buff's in its
/// position's order.
fn aura_client() -> App {
    let mut app = client();
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"
            Auras = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("UNIT_AURA")
            f:RegisterEvent("PLAYER_AURAS_CHANGED")
            f:SetScript("OnEvent", function()
                if event == "UNIT_AURA" then
                    local _, count = UnitBuff(arg1, 1)
                    table.insert(Auras, "UNIT_AURA:" .. arg1 .. ":" .. tostring(count))
                    return
                end
                local cached = ""
                for i = 0, 3 do
                    local at = GetPlayerBuff(i, "HELPFUL")
                    if at < 0 then break end
                    cached = cached .. GetPlayerBuffApplications(at)
                end
                table.insert(Auras, event .. ":" .. cached)
            end)
            "#,
        )
        .unwrap();
    frame(&mut app, vec![vec![values(ME, &[(FLAGS, 0x0800_0000)])]]);
    frame(&mut app, vec![]);
    app
}

/// Slot `slot` holding `spell` at `stacks` (its applications byte plus one), its nibble shown.
fn aura(slot: u16, spell: u32, stacks: u32) -> Vec<(u16, u32)> {
    let shift = 4 * (slot % 8) as u32;
    let byte = 8 * (slot % 4) as u32;
    vec![
        (AURA + slot, spell),
        (AURAFLAGS + slot / 8, SHOWN << shift),
        (APPLICATIONS + slot / 4, (stacks - 1) << byte),
    ]
}

/// Two aura packets in one frame: each `UNIT_AURA` listener reads the first buff its own packet
/// left (`0x519500` walks the fields at the call), not the frame's last twice.
#[test]
fn each_unit_aura_listener_reads_its_own_packets_buffs() {
    let mut app = aura_client();
    let seen = frame(
        &mut app,
        vec![
            vec![values(ME, &aura(0, 100, 1))],
            vec![values(ME, &aura(0, 100, 3))],
        ],
    );
    let unit_aura: Vec<&String> = seen.iter().filter(|s| s.starts_with("UNIT_AURA")).collect();
    assert_eq!(unit_aura, ["UNIT_AURA:player:1", "UNIT_AURA:player:3"]);
}

/// The aura watcher signals once per packet that moves the aura fields, each listener reading
/// the cache that packet's rebuild left (`0x4e4140` → `0x4e4170` → `0x4e437f`).
#[test]
fn player_auras_changed_fires_per_packet_on_its_own_cache() {
    let mut app = aura_client();
    let seen = frame(
        &mut app,
        vec![
            vec![values(ME, &aura(0, 100, 1))],
            vec![values(ME, &aura(0, 100, 3))],
        ],
    );
    let changed: Vec<&String> = seen
        .iter()
        .filter(|s| s.starts_with("PLAYER_AURAS_CHANGED"))
        .collect();
    assert_eq!(
        changed,
        ["PLAYER_AURAS_CHANGED:1", "PLAYER_AURAS_CHANGED:3"]
    );
}

/// The cache is rebuilt at each packet, so its order is the order the auras arrived in packet by
/// packet: a buff dropped by one packet and back in the next takes the last place behind one the
/// first packet added (`0x4e421b`, then the append at `0x4e4360`), as a rebuild over the frame's
/// last state alone would not.
#[test]
fn the_buff_cache_takes_each_packets_auras_in_turn() {
    let mut app = aura_client();
    frame(&mut app, vec![vec![values(ME, &aura(0, 100, 1))]]);
    // Slot 0 emptied and slot 1 given 200 at two stacks; then slot 0 given 100 again.
    let drop_and_add = [
        (AURA, 0),
        (AURA + 1, 200),
        (AURAFLAGS, SHOWN << 4),
        (APPLICATIONS, 1 << 8),
    ];
    let back = [(AURA, 100), (AURAFLAGS, SHOWN | SHOWN << 4)];
    let seen = frame(
        &mut app,
        vec![vec![values(ME, &drop_and_add)], vec![values(ME, &back)]],
    );
    assert_eq!(
        seen.last().map(String::as_str),
        Some("PLAYER_AURAS_CHANGED:21"),
        "the returning buff behind the new one: {seen:?}"
    );
}

/// The aura watcher signals in its packet's notify pass among the bridge's `UNIT_AURA` watches,
/// at the first field of its range the block visits (`0x4654a4` → `0x465570`). The first packet
/// moves a spell id: the bridge's watch over the ids (id 41) and the watcher both run at that
/// field, the bridge's first, as the player's post-init registers it (`0x5dec09` → `0x5fbb21` →
/// `0x60c65c` → `0x51bbb0`) ahead of the arrival that registers the watcher (`0x5dec7b` →
/// `0x5deb60` → `0x4908c0` → `0x4e4100`). The second moves only a stack count: the watcher runs
/// at the ids' field it visits, the bridge's applications watch (id 107) at the count's.
#[test]
fn the_aura_watcher_signals_at_its_place_in_its_packet() {
    let mut app = aura_client();
    let seen = frame(
        &mut app,
        vec![
            vec![values(ME, &aura(0, 100, 1))],
            vec![values(ME, &aura(0, 100, 3))],
        ],
    );
    assert_eq!(
        seen,
        [
            "UNIT_AURA:player:1",
            "PLAYER_AURAS_CHANGED:1",
            "PLAYER_AURAS_CHANGED:3",
            "UNIT_AURA:player:3"
        ]
    );
}
