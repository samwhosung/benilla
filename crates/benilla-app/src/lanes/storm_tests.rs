//! A raid's update storm, timed through the whole client's frame: the cost a unit's packets put
//! on the frame, for a change to the packet path to be weighed by.

use benilla_protocol::field::{FIELD_UNIT_HEALTH, FIELD_UNIT_MAXHEALTH};
use benilla_protocol::messages::GroupMemberEntry;
use benilla_protocol::{EntityKind, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};

use super::unit_view_tests::{created, values, wire_packets};
use crate::net::ClientCommand;

const ME: u64 = 0x7;
const WOLF: u64 = 0xF130_0000_0044_0001;

/// The whole client, headless, in the world with its frame run: the packets go in through the
/// inbound channel, and each frame is the client's own `Update`.
struct Client {
    app: App,
    inbound: Sender<SessionEvent>,
    _sent: Receiver<ClientCommand>,
}

impl Client {
    /// One frame of the client's, with `packets` in the inbound channel, each one packet.
    fn frame(&mut self, packets: Vec<Vec<SessionEvent>>) {
        for ev in wire_packets(packets) {
            self.inbound.send(ev).expect("the inbound channel");
        }
        self.app.update();
    }

    /// Run `lua` in the VM.
    fn lua(&mut self, lua: &str) {
        crate::script_calls::with_hosted_vm(self.app.world_mut(), |s| {
            s.expect("a VM").run(lua).expect("the chunk runs");
        });
    }
}

/// The client in the world as [`ME`], holding [`WOLF`] (100 of 100 health) as its target, with a
/// VM that has run `spy`.
fn client(spy: &str) -> Client {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    app.insert_resource(bevy::ecs::error::DefaultErrorHandler(
        bevy::ecs::error::ignore,
    ));
    let (tx, sent) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    let (inbound, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetEvents::for_test(rx));
    // The start-up frame first, which seats a VM of its own.
    app.update();
    app.insert_non_send_resource(UiScript::new().expect("a VM"));
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    app.world_mut().resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    let mut c = Client {
        app,
        inbound,
        _sent: sent,
    };
    let mob = |hp: u32| [(FIELD_UNIT_HEALTH, hp), (FIELD_UNIT_MAXHEALTH, 100)];
    c.frame(vec![
        vec![created(ME, EntityKind::Player, &mob(100))],
        vec![created(WOLF, EntityKind::Unit, &mob(100))],
    ]);
    let world = c.app.world_mut();
    let wolf = world.resource::<crate::net::GuidIndex>().0[&WOLF];
    *world.resource_mut::<crate::target::Selection>() = crate::target::Selection {
        target: Some(wolf),
        guid: Some(WOLF),
        ..Default::default()
    };
    c.frame(vec![]);
    c.lua(spy);
    c
}

/// A raid's update storm, timed through the whole client's frame: 40 units, each with three
/// health packets a frame, heard by a raid frame's worth of `UNIT_HEALTH` listeners.
///
/// Run with `cargo nextest run -p benilla-app --lib raid_storm_cost --run-ignored only
/// --no-capture`.
#[test]
#[ignore = "a timing, not a check"]
fn raid_storm_cost() {
    let mut c = client(
        r#"for i = 1, 40 do
            local f = CreateFrame("Frame")
            f.unit = "raid" .. i
            f:RegisterEvent("UNIT_HEALTH")
            f:SetScript("OnEvent", function()
                if arg1 == this.unit then this.hp = UnitHealth(arg1) end
            end)
        end"#,
    );
    let members = raid(&mut c);
    let units: Vec<u64> = std::iter::once(ME).chain(members.iter().copied()).collect();
    let storm = |frame: u32| {
        (0..3u32)
            .flat_map(|round| {
                units.iter().map(move |&g| {
                    vec![values(
                        g,
                        &[(FIELD_UNIT_HEALTH, 1 + (frame * 3 + round) % 99)],
                    )]
                })
            })
            .collect::<Vec<_>>()
    };
    for f in 0..30 {
        c.frame(storm(f));
    }
    let mut times = Vec::new();
    for f in 30..330 {
        let packets = storm(f);
        let t = std::time::Instant::now();
        c.frame(packets);
        times.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(f64::total_cmp);
    let mean = times.iter().sum::<f64>() / times.len() as f64;
    eprintln!(
        "raid storm: {} packets a frame, {} frames: mean {mean:.3} ms, median {:.3} ms, p90 {:.3} ms",
        units.len() * 3,
        times.len(),
        times[times.len() / 2],
        times[times.len() * 9 / 10],
    );
    let heard: i64 = crate::script_calls::eval_in_vm(
        c.app.world(),
        "local n = 0 for i = 1, 40 do if UnitHealth('raid' .. i) > 0 then n = n + 1 end end return n",
    );
    eprintln!("raid units with health: {heard}");
}

/// Our raid of 39 others, each held at 100 of 100 health; their guids.
fn raid(c: &mut Client) -> Vec<u64> {
    let members: Vec<u64> = (0..39).map(|i| 0x100 + i).collect();
    let roster = SessionEvent::GroupList {
        group_type: crate::ui_party::GROUPTYPE_RAID,
        own_flags: 0,
        members: members
            .iter()
            .enumerate()
            .map(|(i, &guid)| GroupMemberEntry {
                name: format!("M{guid:x}"),
                guid,
                status: 1,
                flags: ((i + 1) / 5) as u8,
            })
            .collect(),
        leader: ME,
        loot: None,
    };
    let mob = [(FIELD_UNIT_HEALTH, 100), (FIELD_UNIT_MAXHEALTH, 100)];
    let mut setup = vec![vec![roster]];
    setup.extend(
        members
            .iter()
            .map(|&g| vec![created(g, EntityKind::Player, &mob)]),
    );
    c.frame(setup);
    members
}

/// The `Unit*` getters' per-call cost with the host lent: 10k `UnitHealth` calls over `raid1..40`.
///
/// Run with `cargo nextest run -p benilla-app --lib unit_getter_cost --run-ignored only
/// --no-capture`.
#[test]
#[ignore = "a timing, not a check"]
fn unit_getter_cost() {
    let mut c = client("");
    raid(&mut c);
    c.frame(vec![]);
    let chunk = r#"
        local tokens = {}
        for i = 1, 40 do tokens[i] = "raid" .. i end
        local sum = 0
        for n = 1, 10000 do sum = sum + UnitHealth(tokens[math.mod(n, 40) + 1]) end
        return sum"#;
    let mut median = |live: bool| {
        let mut times = Vec::new();
        for _ in 0..50 {
            let t = std::time::Instant::now();
            let sum: i64 = if live {
                crate::script_calls::with_hosted_vm(c.app.world_mut(), |s| {
                    s.expect("a VM").eval(chunk).expect("runs")
                })
            } else {
                crate::script_calls::eval_in_vm(c.app.world(), chunk)
            };
            times.push(t.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(sum, 10000 * 100);
        }
        times.sort_by(f64::total_cmp);
        times[times.len() / 2]
    };
    let (live, snapshot) = (median(true), median(false));
    eprintln!(
        "unit getters: 10k UnitHealth calls, median {live:.3} ms with the host lent, {snapshot:.3} ms \
         off the snapshots alone"
    );
}
