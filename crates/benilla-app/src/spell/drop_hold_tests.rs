//! `0x495920`: losing control and a world port both put the cursor's payload back and cancel the
//! cast (`ClearCursor(1, 1)`, `CancelCast(1, 1, 0x1c)`).

use benilla_protocol::field::FIELD_PLAYER_FIELD_COINAGE;
use benilla_protocol::messages::ObjectType;
use benilla_protocol::{ObjectFields, SessionEvent};
use bevy::prelude::*;
use crossbeam_channel::Receiver;

use crate::net::{ClientCommand, Guid, GuidIndex, NetCommands, ObjectStore, SelfGuid, SelfPlayer};
use crate::spell::PendingCast;

const ME: u64 = 0x10;
const FIREBALL: u32 = 133;

struct Client {
    app: App,
    sent: Receiver<ClientCommand>,
}

/// The whole client headless with our player streamed, holding 300 copper on the cursor, a
/// Fireball in flight, and a listener that logs the cursor's money beside each event.
fn client() -> Client {
    let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
    let (tx, sent) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx));
    let world = app.world_mut();
    world.resource_mut::<SelfGuid>().0 = Some(ME);
    world.resource_mut::<crate::net::NetStatus>().connected = true;
    crate::script_calls::insert_test_vm(world);
    let me = world
        .spawn((
            SelfPlayer,
            Guid(ME),
            Transform::default(),
            ObjectStore(ObjectFields::from_pairs(&[(2, 0x19)]).into_created(ObjectType::Player)),
        ))
        .id();
    world.resource_mut::<GuidIndex>().0.insert(ME, me);
    let mut c = Client { app, sent };
    let block = SessionEvent::ObjectValues {
        guid: ME,
        fields: ObjectFields::from_pairs(&[(FIELD_PLAYER_FIELD_COINAGE, 500)]),
    };
    crate::net::handlers::dispatch(c.world(), crate::net::wire([block]));
    crate::lanes::fire_in_order(c.world());
    c.lua(
        r#"SEEN = {} local f = CreateFrame("Frame")
        for _, e in ipairs({"CURSOR_UPDATE", "PLAYER_CONTROL_LOST", "PLAYER_CONTROL_GAINED"}) do
            f:RegisterEvent(e)
        end
        f:SetScript("OnEvent", function() table.insert(SEEN, event .. " " .. GetCursorMoney()) end)"#,
    );
    c.hold();
    c
}

impl Client {
    fn world(&mut self) -> &mut World {
        self.app.world_mut()
    }

    fn lua(&mut self, script: &str) {
        crate::script_calls::with_hosted_vm(self.world(), |s| s.expect("a VM").run(script))
            .expect("the chunk runs");
    }

    /// 300 copper on the cursor and a Fireball in flight, nothing logged or sent yet.
    fn hold(&mut self) {
        self.lua("PickupPlayerMoney(300) SEEN = {}");
        self.world()
            .resource_mut::<PendingCast>()
            .push_cast(FIREBALL, None, None);
        self.sent();
    }

    fn seen(&mut self) -> Vec<String> {
        crate::script_calls::eval_in_vm(self.world(), "local s = SEEN SEEN = {} return s")
    }

    fn sent(&self) -> Vec<String> {
        self.sent.try_iter().map(|c| format!("{c:?}")).collect()
    }
}

/// `SMSG_CLIENT_CONTROL_UPDATE` for our own mover: its fall drops the cursor's payload
/// (`ClearCursor(1, 1)`, whose `CURSOR_UPDATE` reads it empty) and the cast in flight
/// (`CancelCast(1, 1, 0x1c)`), in that order and once, in the packet (`0x495901` → `0x495920`);
/// a store that changes nothing, regaining control and another unit's verdict drop nothing. The
/// controller applies the flag later, so the packets of one frame are told apart by their own
/// order.
#[test]
fn losing_control_drops_the_cursor_and_the_cast_in_its_packet() {
    let mut c = client();
    let control = |c: &mut Client, mover: u64, allow_move: bool| {
        let ev = SessionEvent::ClientControl { mover, allow_move };
        crate::net::handlers::dispatch(c.world(), crate::net::wire([ev]));
    };
    control(&mut c, 0x99, false);
    assert_eq!((c.seen(), c.sent()), (vec![], vec![]), "another unit's");
    control(&mut c, ME, false);
    assert_eq!(c.seen(), ["CURSOR_UPDATE 0"]);
    assert_eq!(c.sent(), [format!("CancelCast {{ spell_id: {FIREBALL} }}")]);
    assert_eq!(c.world().resource::<PendingCast>().current(), None);

    // The cursor refuses a payload while control is down, so a cast stands for what is held.
    c.world()
        .resource_mut::<PendingCast>()
        .push_cast(FIREBALL, None, None);
    control(&mut c, ME, false);
    assert_eq!((c.seen(), c.sent()), (vec![], vec![]), "the flag was down");
    control(&mut c, ME, true);
    assert_eq!(
        (c.seen(), c.sent()),
        (vec![], vec![]),
        "regaining drops nothing"
    );
    assert_eq!(
        c.world().resource::<PendingCast>().current(),
        Some(FIREBALL)
    );
    control(&mut c, ME, false);
    assert_eq!(
        c.sent(),
        [format!("CancelCast {{ spell_id: {FIREBALL} }}")],
        "a new fall"
    );
}

/// Both arms of `SMSG_TRANSFER_PENDING`'s handler reach `0x495920` (`0x40194c`).
#[test]
fn a_transfer_pending_drops_the_cursor_and_the_cast() {
    for transport_entry in [None, Some(176_080)] {
        let mut c = client();
        let pending = SessionEvent::TransferPending {
            map_id: 36,
            transport_entry,
            from_map: None,
        };
        crate::net::handlers::dispatch(c.world(), crate::net::wire([pending]));
        assert_eq!(c.seen(), ["CURSOR_UPDATE 0"], "{transport_entry:?}");
        assert_eq!(c.sent(), [format!("CancelCast {{ spell_id: {FIREBALL} }}")]);
    }
}
