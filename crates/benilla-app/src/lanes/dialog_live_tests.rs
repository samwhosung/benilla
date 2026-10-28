//! The dialogs' getters as a listener reads them, and their events at their packets. The getters
//! read the dialog globals at the call: the service latches (`0x48d210`, `0x48d1c0`), the boot and
//! spirit-healer deadlines (`0x48b620`, `[0xb4e338]`), the meeting stone's area and text
//! (`0x4ca570`, `0x4ca5b0`), the summon latch (`0x48b660`-`0x48b720`) and the instance terms
//! (`0x48a750`, `0x495c90`). The events fire inside the handler that raises them: the innkeeper's
//! (`0x5dfdc0`), the pet trainer's (`0x5dfba0`), the boot's (`0x496340`), the summon's (`0x4963ce`),
//! the stone's (`0x4ca38f`) and a name answer's held challenge.

use benilla_formats::{AreaTableCatalog, AreaTableRow};
use benilla_protocol::field::{FIELD_UNIT_HEALTH, FIELD_UNIT_MAXHEALTH};
use benilla_protocol::{EntityKind, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};

use super::unit_view_tests::{created, wire_packets};
use crate::net::ClientCommand;
use crate::ui_dialog_verbs::DialogSignal;

const ME: u64 = 0x7;
/// An innkeeper, a pet trainer and a talent trainer in one, standing on us.
const NPC: u64 = 0xF130_0000_0000_0051;
const SUMMONER: u64 = 0x31;
const STORMWIND: u32 = 1519;
const DEADMINES: u32 = 36;

/// The whole client, headless, in the world with its frame run.
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

    /// `chunk`'s answer through the lent host when `live`, else off the VM's copies alone.
    fn eval<T: benilla_ui::mlua::FromLuaMulti>(&mut self, live: bool, chunk: &str) -> T {
        if live {
            crate::script_calls::with_hosted_vm(self.app.world_mut(), |s| {
                s.expect("a VM").eval::<T>(chunk).expect("runs")
            })
        } else {
            crate::script_calls::eval_in_vm(self.app.world(), chunk)
        }
    }

    /// What the spy heard since the last call.
    fn seen(&mut self) -> Vec<String> {
        self.eval(false, "local s = Seen Seen = {} return s")
    }

    /// `packets` run through their handlers with no frame: the world as the handlers leave it,
    /// which no feed has pushed.
    fn handle(&mut self, packets: Vec<SessionEvent>) {
        crate::net::handlers::dispatch(self.app.world_mut(), packets);
    }
}

/// The client in the world with the NPC beside us, Stormwind in the area table, the Deadmines a
/// party dungeon, and a spy whose line for each dialog event and message carries its name and
/// first argument.
fn client() -> Client {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    app.insert_resource(bevy::ecs::error::DefaultErrorHandler(
        bevy::ecs::error::ignore,
    ));
    let (tx, sent) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    let (inbound, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetEvents::for_test(rx));
    app.update();
    let script = UiScript::new().expect("a VM");
    script
        .run(
            r#"
            Seen = {}
            MEETINGSTONE_TOOLTIP = "Looking for more for %s"
            ERR_RAID_GROUP_ONLY = "raid only"
            HOME_INN = "your inn"
            local f = CreateFrame("Frame")
            for _, e in ipairs({"CONFIRM_BINDER", "CONFIRM_PET_UNLEARN", "CONFIRM_SUMMON",
                    "CONFIRM_TALENT_WIPE", "INSTANCE_BOOT_START", "INSTANCE_BOOT_STOP",
                    "MEETINGSTONE_CHANGED", "DUEL_REQUESTED", "UI_ERROR_MESSAGE",
                    "CHAT_MSG_SYSTEM"}) do
                f:RegisterEvent(e)
            end
            f:SetScript("OnEvent", function()
                table.insert(Seen, event .. ":" .. tostring(arg1))
            end)
            "#,
        )
        .unwrap();
    app.insert_non_send_resource(script);
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    let row = |name: &str| AreaTableRow {
        map_id: 0,
        zone_id: 0,
        explore_flag: 0,
        flags: 0,
        faction_group_mask: 0,
        exploration_level: 0,
        name: name.into(),
    };
    app.insert_resource(crate::area::AreaTableRes(AreaTableCatalog::from_rows(
        vec![(STORMWIND, row("Stormwind City"))],
    )));
    app.insert_resource(benilla_assets::MapCatalogRes(
        benilla_formats::MapCatalog::from_rows([(DEADMINES, "Deadmines", 0)])
            .with_instance_types([(DEADMINES, 1)]),
    ));
    app.world_mut().resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    let mut c = Client {
        app,
        inbound,
        _sent: sent,
    };
    let unit = |guid, kind| {
        created(
            guid,
            kind,
            &[(FIELD_UNIT_HEALTH, 100), (FIELD_UNIT_MAXHEALTH, 100)],
        )
    };
    c.frame(vec![
        vec![unit(ME, EntityKind::Player)],
        vec![unit(NPC, EntityKind::Unit)],
    ]);
    c.frame(vec![]);
    c.frame(vec![]);
    c.seen();
    c
}

/// Every getter of the dialogs, with the host lent, reads what the handlers and the world left,
/// where the VM's copies, which no feed pushes, read the empty dialogs.
#[test]
fn the_dialog_getters_read_the_world_through_the_host() {
    let mut c = client();
    c.handle(vec![
        SessionEvent::BinderConfirm { binder: NPC },
        SessionEvent::PetUnlearnConfirm {
            trainer: NPC,
            cost: 500,
        },
        SessionEvent::RaidGroupOnly {
            delay_ms: 60_500,
            reason: 0,
        },
        SessionEvent::MeetingStoneSetQueue {
            area: STORMWIND,
            status: 1,
        },
        SessionEvent::SummonRequest {
            summoner: SUMMONER,
            zone: STORMWIND,
            delay_ms: 120_000,
        },
    ]);
    let world = c.app.world_mut();
    world
        .resource_mut::<crate::names::NameCache>()
        .insert_player(SUMMONER, "Warlock".into(), None);
    {
        let mut spirit = world.resource_mut::<crate::ui_dialog_verbs::AreaSpiritHealer>();
        spirit.set_healer(Some(NPC));
        spirit.on_time(NPC, 30_500, std::time::Instant::now());
    }
    world.insert_resource(benilla_world::world_map::CurrentMap(DEADMINES));

    let chunk = "return CheckBinderDist(), CheckPetUntrainerDist(), \
                 GetInstanceBootTimeRemaining(), GetAreaSpiritHealerTime(), \
                 IsInMeetingStoneQueue(), GetMeetingStoneStatusText(), \
                 GetSummonConfirmSummoner(), GetSummonConfirmAreaName(), \
                 GetSummonConfirmTimeLeft(), IsInInstance()";
    type Reads = (
        Option<i64>,
        Option<i64>,
        i64,
        i64,
        Option<i64>,
        Option<String>,
        String,
        String,
        f64,
        Option<i64>,
    );
    let live: Reads = c.eval(true, chunk);
    assert_eq!(live.0, Some(1), "the bind question is latched");
    assert_eq!(live.1, Some(1), "the unlearn question is latched");
    assert!((59..=60).contains(&live.2), "the boot clock: {}", live.2);
    assert!((29..=30).contains(&live.3), "the wave clock: {}", live.3);
    assert_eq!(live.4, Some(1), "queued at the stone");
    assert_eq!(
        live.5.as_deref(),
        Some("Looking for more for Stormwind City")
    );
    assert_eq!(live.6, "Warlock", "the summoner's name off the cache");
    assert_eq!(live.7, "Stormwind City");
    assert!((119.0..=120.0).contains(&live.8), "{}", live.8);
    assert_eq!(live.9, Some(1), "inside a party dungeon");

    let snapshot: Reads = c.eval(false, chunk);
    assert_eq!(
        snapshot,
        (
            None,
            None,
            0,
            0,
            None,
            None,
            String::new(),
            String::new(),
            0.0,
            None
        ),
        "the VM's copies, pushed by nobody"
    );
}

/// Each dialog's event goes on its lane at its handler, stamped there: a frame's packets fire in
/// their order across the lanes, the talent trainer's notice between them, and a boot's line
/// ahead of its stop (`0x5e4914`).
#[test]
fn the_dialog_events_fire_at_their_packets() {
    let mut c = client();
    c.handle(vec![
        SessionEvent::SummonRequest {
            summoner: SUMMONER,
            zone: STORMWIND,
            delay_ms: 120_000,
        },
        SessionEvent::TalentWipeConfirm {
            trainer: NPC,
            cost: 100,
        },
        SessionEvent::BinderConfirm { binder: NPC },
    ]);
    let dialogs = c
        .app
        .world_mut()
        .resource_mut::<crate::ui_dialog_verbs::DialogLane>()
        .take_stamped();
    let notices: Vec<_> = c
        .app
        .world_mut()
        .resource_mut::<crate::field_watch::NoticeLane>()
        .drain_stamped()
        .collect();
    assert_eq!(dialogs.len(), 2);
    assert_eq!(
        dialogs[0].1,
        DialogSignal::Event("CONFIRM_SUMMON".into(), vec![])
    );
    assert_eq!(
        dialogs[1].1,
        DialogSignal::ConfirmBinder { leaf: None },
        "named by the area under the player"
    );
    assert_eq!(notices.len(), 1);
    assert!(
        dialogs[0].0 < notices[0].0 && notices[0].0 < dialogs[1].0,
        "each stamped at its handler"
    );

    c.frame(vec![
        vec![SessionEvent::SummonRequest {
            summoner: SUMMONER,
            zone: STORMWIND,
            delay_ms: 120_000,
        }],
        vec![SessionEvent::TalentWipeConfirm {
            trainer: NPC,
            cost: 100,
        }],
        vec![SessionEvent::BinderConfirm { binder: NPC }],
        vec![SessionEvent::PetUnlearnConfirm {
            trainer: NPC,
            cost: 500,
        }],
        vec![SessionEvent::RaidGroupOnly {
            delay_ms: 0,
            reason: 1,
        }],
        vec![SessionEvent::MeetingStoneSetQueue {
            area: STORMWIND,
            status: 2,
        }],
    ]);
    assert_eq!(
        c.seen(),
        [
            "CONFIRM_SUMMON:nil",
            "CONFIRM_TALENT_WIPE:100",
            "CONFIRM_BINDER:your inn",
            "CONFIRM_PET_UNLEARN:500",
            "UI_ERROR_MESSAGE:raid only",
            "INSTANCE_BOOT_STOP:nil",
            "MEETINGSTONE_CHANGED:nil",
        ]
    );
}

/// The pet trainer's question passes the same gate as the innkeeper's (`0x5dfbde`, `0x5dfc2d`)
/// before it latches (`0x5dfc41`): a trainer out of range, or with no unit, latches and signals
/// nothing.
#[test]
fn the_unlearn_question_drops_past_the_range_gate() {
    let mut c = client();
    let npc = c.app.world().resource::<crate::net::GuidIndex>().0[&NPC];
    c.app
        .world_mut()
        .get_mut::<Transform>(npc)
        .unwrap()
        .translation
        .x = 10.0;
    let ask = |trainer| SessionEvent::PetUnlearnConfirm { trainer, cost: 500 };
    c.handle(vec![ask(NPC), ask(0xF130_0000_0000_0099)]);
    assert!(c
        .app
        .world_mut()
        .resource_mut::<crate::ui_dialog_verbs::DialogLane>()
        .take()
        .is_empty());
    assert_eq!(
        c.eval::<Option<i64>>(true, "return CheckPetUntrainerDist()"),
        None
    );
    c.app
        .world_mut()
        .get_mut::<Transform>(npc)
        .unwrap()
        .translation
        .x = 5.0;
    c.handle(vec![ask(NPC)]);
    assert_eq!(
        c.eval::<Option<i64>>(true, "return CheckPetUntrainerDist()"),
        Some(1),
        "5 yd is inside 5.56"
    );
}

/// Walking away from an innkeeper, a pet trainer or a talent trainer with its question open closes
/// nothing: no engine path clears the latches (`0xc4d7c0`, `0xc4d7b0`, `0xc4d7a0`, written only
/// at `0x5dfe64`, `0x5dfc41`, `0x5dfa21`), and each poll tests the latched NPC's range at the
/// call (`0x5dff50`, `0x5dfd30`, `0x5dfb10`), so the stock `OnUpdate` hides the dialog
/// (`StaticPopup.lua:1297`, `1313`, `1329`). Out of range the Accept sends nothing; back in range
/// the poll holds again and the Accept answers the latched NPC (`0x5dfdc0`, `0x5dfba0`, `0x5df980`
/// with a zero guid).
#[test]
fn walking_away_keeps_the_service_latches_and_the_polls_test_the_range() {
    use crate::net::ClientCommand as C;
    let mut c = client();
    c.frame(vec![
        vec![SessionEvent::BinderConfirm { binder: NPC }],
        vec![SessionEvent::PetUnlearnConfirm {
            trainer: NPC,
            cost: 0,
        }],
        vec![SessionEvent::TalentWipeConfirm {
            trainer: NPC,
            cost: 0,
        }],
    ]);
    assert_eq!(
        c.seen(),
        [
            "CONFIRM_BINDER:your inn",
            "CONFIRM_PET_UNLEARN:0",
            "CONFIRM_TALENT_WIPE:0"
        ]
    );
    const POLLS: &str =
        "return CheckBinderDist(), CheckPetUntrainerDist(), CheckTalentMasterDist()";
    type Polls = (Option<i64>, Option<i64>, Option<i64>);
    assert_eq!(c.eval::<Polls>(true, POLLS), (Some(1), Some(1), Some(1)));
    let latched = |c: &Client| {
        let w = c.app.world();
        (
            w.resource::<crate::ui_binder::BinderState>().pending(),
            w.resource::<crate::ui_dialog_verbs::PetUnlearnState>()
                .pending(),
            w.resource::<crate::ui_talent_wipe::TalentWipeState>()
                .pending(),
        )
    };
    let place = |c: &mut Client, x: f32| {
        let npc = c.app.world().resource::<crate::net::GuidIndex>().0[&NPC];
        c.app
            .world_mut()
            .get_mut::<Transform>(npc)
            .unwrap()
            .translation
            .x = x;
    };
    let accept = |c: &mut Client| -> Vec<String> {
        let _: () = c.eval(
            true,
            "ConfirmBinder() ConfirmPetUnlearn() ConfirmTalentWipe()",
        );
        c._sent
            .try_iter()
            .filter(|s| {
                matches!(
                    s,
                    C::BinderActivate { .. } | C::PetUnlearn { .. } | C::TalentWipeConfirm { .. }
                )
            })
            .map(|s| format!("{s:?}"))
            .collect()
    };

    place(&mut c, 6.0);
    c.frame(vec![]);
    assert_eq!(
        latched(&c),
        (Some(NPC), Some(NPC), Some(NPC)),
        "nothing closes"
    );
    assert_eq!(c.eval::<Polls>(true, POLLS), (None, None, None));
    assert!(accept(&mut c).is_empty(), "out of range, no Accept sends");

    place(&mut c, 5.0);
    c.frame(vec![]);
    assert_eq!(c.eval::<Polls>(true, POLLS), (Some(1), Some(1), Some(1)));
    assert_eq!(
        accept(&mut c),
        [
            format!("{:?}", C::BinderActivate { binder: NPC }),
            format!("{:?}", C::PetUnlearn { trainer: NPC }),
            format!("{:?}", C::TalentWipeConfirm { trainer: NPC }),
        ]
    );
}

/// A world-enter cascade and a `0x295` in one frame: the reset is the create's, ahead of the
/// frame's later packets, so the rebuild stands (`0x4908c0` inside `0x5dea50`, then `0x4ca385`).
#[test]
fn a_rebuild_in_the_cascades_frame_stands() {
    let mut c = client();
    crate::ui_world_enter::push_arrival(c.app.world_mut());
    c.frame(vec![vec![SessionEvent::MeetingStoneSetQueue {
        area: STORMWIND,
        status: 1,
    }]]);
    assert_eq!(
        c.eval::<Option<String>>(true, "return GetMeetingStoneStatusText()")
            .as_deref(),
        Some("Looking for more for Stormwind City")
    );
}

/// A challenge whose name the cache lacks fires its `DUEL_REQUESTED` at the name's answer, after
/// what the packets between raised (the module's deviation from `0x4d4a72`).
#[test]
fn a_held_challenge_fires_at_its_names_answer() {
    let mut c = client();
    c.frame(vec![
        vec![SessionEvent::DuelRequested {
            arbiter: 0xA1,
            challenger: SUMMONER,
        }],
        vec![SessionEvent::TalentWipeConfirm {
            trainer: NPC,
            cost: 100,
        }],
        vec![SessionEvent::PlayerName {
            guid: SUMMONER,
            name: "Rival".into(),
            race: 1,
            gender: 0,
            class: 1,
        }],
    ]);
    assert_eq!(
        c.seen(),
        ["CONFIRM_TALENT_WIPE:100", "DUEL_REQUESTED:Rival"]
    );
}
