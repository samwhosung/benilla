//! The death getters as a listener reads them, and the death signals at their packets. The
//! getters read the interface's death globals at the call (`GetCorpseRecoveryDelay` `0x48b5d0`);
//! the resurrect offer shows inside its packet or its caster's name answer (`0x5e7bc0`,
//! `0x5e7ca0` → `0x5ded50`), the revive's `PLAYER_ALIVE` inside the health watcher's run
//! (`0x6047ee` → `0x5fef70`), and the reclaim delay's range event inside its handler (`0x4962d0`).

use benilla_protocol::SessionEvent;
use benilla_ui::script::UiScript;
use bevy::prelude::*;

use super::unit_view_tests::{client, created, fall, frame, health, values};

/// [`client`]'s active player.
const ME: u64 = 0x7;
/// A fall's `UNIT_COMBAT` on our dead body, as [`client`]'s spy records it.
const FELL: &str = "UNIT_COMBAT:player:0";
const PRIEST: u64 = 0x51;

/// [`client`] with a spy on the death signals, each with what its listener reads: the offer's
/// name and timer, the corpse's delay.
fn death_client() -> App {
    let mut app = client();
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"
            local f = CreateFrame("Frame")
            for _, e in ipairs({"RESURRECT_REQUEST", "PLAYER_ALIVE", "CORPSE_IN_RANGE",
                    "CONFIRM_XP_LOSS"}) do
                f:RegisterEvent(e)
            end
            f:SetScript("OnEvent", function()
                if event == "RESURRECT_REQUEST" then
                    table.insert(Seen, event .. ":" .. arg1 .. ":" .. tostring(ResurrectHasTimer()))
                elseif event == "CORPSE_IN_RANGE" then
                    table.insert(Seen, event .. ":" .. GetCorpseRecoveryDelay())
                else
                    table.insert(Seen, event)
                end
            end)
            "#,
        )
        .unwrap();
    app
}

/// [`death_client`] after our death's packet, the frame's signals taken.
fn dead_client() -> App {
    let mut app = death_client();
    frame(&mut app, vec![values(ME, &health(0))]);
    app
}

fn offer(name: &str) -> SessionEvent {
    SessionEvent::ResurrectRequest {
        caster: PRIEST,
        name: name.into(),
        sickness: false,
        has_timer: true,
    }
}

/// With no frame after the reclaim delay's packet, `GetCorpseRecoveryDelay` reads the delay
/// through the host; the VM's copy, which nothing pushes, reads none.
#[test]
fn the_recovery_delay_reads_the_stores_at_the_call() {
    let mut app = dead_client();
    crate::net::handlers::dispatch(
        app.world_mut(),
        vec![SessionEvent::CorpseReclaimDelay { delay_ms: 30_000 }],
    );
    let chunk = "return GetCorpseRecoveryDelay()";
    let live: f64 = crate::script_calls::with_hosted_vm(app.world_mut(), |s| {
        s.expect("a VM").eval(chunk).expect("runs")
    });
    assert!((29.0..=30.0).contains(&live), "{live}");
    let copy: f64 = crate::script_calls::eval_in_vm(app.world(), chunk);
    assert_eq!(copy, 0.0);
}

/// An offer with its caster's name on the wire signals at its packet's place, between a fall's
/// `UNIT_COMBAT` on each side, its listener reading the offer's timer.
#[test]
fn resurrect_request_fires_between_neighbouring_packets_events() {
    let mut app = dead_client();
    let seen = frame(&mut app, vec![fall(5), offer("Bob"), fall(5)]);
    assert_eq!(seen, [FELL, "RESURRECT_REQUEST:Bob:1", FELL]);
}

/// A player caster's offer waits on the name its packet asked (`0x5e7c82`): nothing at the offer,
/// then the show inside the name answer's packet.
#[test]
fn a_player_casters_offer_shows_at_its_name_answer() {
    let mut app = dead_client();
    assert!(frame(&mut app, vec![offer("")]).is_empty());
    let named = SessionEvent::PlayerName {
        guid: PRIEST,
        name: "Ann".into(),
        race: 1,
        gender: 0,
        class: 5,
    };
    let seen = frame(&mut app, vec![fall(5), named, fall(5)]);
    assert_eq!(seen, [FELL, "RESURRECT_REQUEST:Ann:1", FELL]);
}

/// The revive's health rising above 0 runs the watcher's arm inside its packet: `PLAYER_ALIVE`,
/// then the bridge's `UNIT_HEALTH`, between the falls around it.
#[test]
fn player_alive_fires_at_the_revives_packet() {
    let mut app = dead_client();
    let seen = frame(&mut app, vec![fall(5), values(ME, &health(1)), fall(5)]);
    assert_eq!(
        seen,
        [
            FELL,
            "PLAYER_ALIVE",
            "UNIT_HEALTH:player:1",
            "UNIT_COMBAT:player:1"
        ]
    );
}

/// A reclaim delay while the range latch is set signals the in-range event again at its packet,
/// its listener reading the new delay (`0x4962da`, `0x49630d`).
#[test]
fn a_reclaim_delay_in_range_signals_at_its_packet() {
    let mut app = dead_client();
    app.world_mut()
        .resource_mut::<crate::death::DeathLatches>()
        .latch_corpse_in_range_for_test();
    let delay = SessionEvent::CorpseReclaimDelay { delay_ms: 30_000 };
    let seen = frame(&mut app, vec![fall(5), delay, fall(5)]);
    assert_eq!(seen.len(), 3, "{seen:?}");
    assert_eq!((seen[0].as_str(), seen[2].as_str()), (FELL, FELL));
    let read: f64 = seen[1]
        .strip_prefix("CORPSE_IN_RANGE:")
        .and_then(|d| d.parse().ok())
        .unwrap_or_else(|| panic!("{seen:?}"));
    assert!((29.0..=30.0).contains(&read), "{read}");
}

/// A spirit healer's confirm signals `CONFIRM_XP_LOSS` at its packet's place for a ghost with the
/// healer in reach (`0x5df74a`, `0x5df7c1`, `0x5df837`), and nothing for the living.
#[test]
fn confirm_xp_loss_fires_at_its_packet_for_a_ghost_in_reach() {
    use benilla_protocol::field::{FIELD_PLAYER_FLAGS, FIELD_UNIT_HEALTH, FIELD_UNIT_MAXHEALTH};
    const HEALER: u64 = 0xF130_0000_1772_0050;
    let confirm = || SessionEvent::SpiritHealerConfirm { npc: HEALER };
    let mut app = death_client();
    let healer = created(
        HEALER,
        benilla_protocol::EntityKind::Unit,
        &[(FIELD_UNIT_HEALTH, 100), (FIELD_UNIT_MAXHEALTH, 100)],
    );
    frame(&mut app, vec![healer]);
    let alive = "UNIT_COMBAT:player:100";
    assert_eq!(
        frame(&mut app, vec![fall(5), confirm(), fall(5)]),
        [alive, alive],
        "alive: no question"
    );
    frame(
        &mut app,
        vec![values(
            ME,
            &[(FIELD_UNIT_HEALTH, 1), (FIELD_PLAYER_FLAGS, 0x10)],
        )],
    );
    let ghost = "UNIT_COMBAT:player:1";
    assert_eq!(
        frame(&mut app, vec![fall(5), confirm(), fall(5)]),
        [ghost, "CONFIRM_XP_LOSS", ghost]
    );
}
