//! The arrival lane ([`crate::ui_world_enter`]) through the whole client: the arrival's signals fire
//! at its create's stamp, between the packets around it (`0x5dec7b`, inside the update packet), and
//! its listeners read the world as each step leaves it.

use std::collections::HashMap;

use benilla_formats::{SpellCatalog, SpellDisplay};
use benilla_protocol::field::{FIELD_UNIT_HEALTH, FIELD_UNIT_MAXHEALTH};
use benilla_protocol::messages::{guild_event, ChatMessage, GuildEventNotice, CHAT_MSG_SYSTEM};
use benilla_protocol::{EntityKind, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};

use super::unit_view_tests::{created, wire_packets};
use crate::net::ClientCommand;

const ME: u64 = 0x7;
/// Three stance-bar spells, by `StanceBarOrder`: the warded form, then Bear, then Cat.
const BEAR: u32 = 90_021;
const CAT: u32 = 90_022;
const WARDED: u32 = 90_023;
/// A spell on no bar.
const FIREBALL: u32 = 133;

/// The whole client, headless, logged in before our create.
struct Client {
    app: App,
    inbound: Sender<SessionEvent>,
    sent: Receiver<ClientCommand>,
}

impl Client {
    /// One frame, with `packets` in the inbound channel, each one packet.
    fn frame(&mut self, packets: Vec<Vec<SessionEvent>>) {
        for ev in wire_packets(packets) {
            self.inbound.send(ev).expect("the inbound channel");
        }
        self.app.update();
    }

    /// What the spy heard.
    fn seen(&mut self) -> Vec<String> {
        crate::script_calls::eval_in_vm(self.app.world(), "local s = Seen Seen = {} return s")
    }

    /// The guids the name queries sent since the last call asked for.
    fn name_queries(&mut self) -> Vec<u64> {
        self.sent
            .try_iter()
            .filter_map(|c| match c {
                ClientCommand::NameQuery { guid } => Some(guid),
                _ => None,
            })
            .collect()
    }
}

fn named(guid: u64, name: &str) -> SessionEvent {
    SessionEvent::PlayerName {
        guid,
        name: name.into(),
        race: 1,
        gender: 0,
        class: 1,
    }
}

fn spells() -> crate::ui_action::Spells {
    let form = |id: u32, form: u32, order: i32| {
        (
            id,
            SpellDisplay {
                id,
                name: format!("Form {id}"),
                shapeshift_form: Some(form),
                stance_bar_order: order,
                ..Default::default()
            },
        )
    };
    let fireball = SpellDisplay {
        id: FIREBALL,
        name: "Fireball".into(),
        ..Default::default()
    };
    crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(
            [
                form(BEAR, 5, 1),
                form(CAT, 1, 2),
                form(WARDED, 6, 0),
                (FIREBALL, fireball),
            ]
            .into_iter()
            .collect::<HashMap<_, _>>(),
        ),
        ..crate::ui_action::Spells::empty_for_tests()
    }
}

/// The client logged in as [`ME`], our create not yet arrived, the chat display latched as from
/// start-up (`0x8435fc`), and `spy` run in its VM.
fn client(spy: &str) -> Client {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    app.insert_resource(bevy::ecs::error::DefaultErrorHandler(
        bevy::ecs::error::ignore,
    ));
    let (tx, sent) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    let (inbound, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetEvents::for_test(rx));
    app.update();
    app.insert_resource(spells());
    let script = UiScript::new().expect("a VM");
    script.run(spy).expect("the spy");
    app.insert_non_send_resource(script);
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    app.world_mut().resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    app.world().resource::<crate::ui_chat::ChatLog>().latch();
    Client { app, inbound, sent }
}

fn me() -> SessionEvent {
    created(
        ME,
        EntityKind::Player,
        &[(FIELD_UNIT_HEALTH, 100), (FIELD_UNIT_MAXHEALTH, 100)],
    )
}

/// The arrival learns the initial spells one at a time in their wire order (`0x5dea96`-
/// `0x5deab9`), each form's add signalling `UPDATE_SHAPESHIFT_FORMS` in place (`0x4b2904`), so the
/// k-th listener counts k forms, in bar order as that learn left the list (`0x4b28f7`); a spell on
/// no bar signals nothing.
#[test]
fn the_kth_form_signal_reads_k_forms() {
    let mut c = client(
        r#"Seen = {}
        local f = CreateFrame("Frame")
        f:RegisterEvent("UPDATE_SHAPESHIFT_FORMS")
        f:SetScript("OnEvent", function()
            local names = {}
            for i = 1, GetNumShapeshiftForms() do
                local _, name = GetShapeshiftFormInfo(i)
                names[i] = name
            end
            table.insert(Seen, GetNumShapeshiftForms() .. ":" .. table.concat(names, ","))
        end)"#,
    );
    c.frame(vec![
        vec![SessionEvent::SpellBook {
            spell_ids: vec![CAT, FIREBALL, BEAR, WARDED],
            cooldowns: Vec::new(),
        }],
        vec![me()],
    ]);
    assert_eq!(
        c.seen(),
        [
            "1:Form 90022",
            "2:Form 90021,Form 90022",
            "3:Form 90023,Form 90021,Form 90022",
        ]
    );
}

/// The arrival's signals fire at its create's stamp: an event a packet ahead of the create raises
/// at once (`GUILD_MOTD`, `0x5e728d`) before `PLAYER_ENTERING_WORLD`, and a line from a packet
/// after it behind. A system line ahead of the create is queued behind the display latch
/// (`0x49db5c` → `0x49cae0`) and shown by the cascade's chat release (`0x490974`), after both
/// login events, so it follows `PLAYER_ENTERING_WORLD` too, ahead of the line after the create.
#[test]
fn the_arrival_fires_between_the_packets_around_its_create() {
    let mut c = client(
        r#"Seen = {}
        local f = CreateFrame("Frame")
        for _, e in ipairs({"PLAYER_ENTERING_WORLD", "GUILD_MOTD", "CHAT_MSG_SYSTEM"}) do
            f:RegisterEvent(e)
        end
        f:SetScript("OnEvent", function()
            table.insert(Seen, event .. ":" .. tostring(arg1))
        end)"#,
    );
    let system = |text: &str| {
        SessionEvent::Chat(ChatMessage {
            chat_type: CHAT_MSG_SYSTEM,
            language: 0,
            sender_guid: 0,
            target_guid: 0,
            sender_name: None,
            channel: None,
            text: text.into(),
            chat_tag: 0,
        })
    };
    c.frame(vec![
        vec![SessionEvent::GuildEvent(GuildEventNotice {
            event: guild_event::MOTD,
            params: vec!["motd".into()],
            guid: None,
        })],
        vec![system("before")],
        vec![me()],
        vec![system("after")],
    ]);
    assert_eq!(
        c.seen(),
        [
            "GUILD_MOTD:motd",
            "PLAYER_ENTERING_WORLD:nil",
            "CHAT_MSG_SYSTEM:before",
            "CHAT_MSG_SYSTEM:after",
        ]
    );
}

/// The cascade's lists call `0x4e7f00` (`0x4909e7`) recounts the looking-for-group types and
/// rebuilds the quest list, each signalling `UPDATE_LFG_TYPES` (`0x4e8b0e`, `0x4e8c70`): twice,
/// after the quest log's rebuild (`0x4909a1`), ahead of the skill list's (`0x4909f1`).
#[test]
fn the_cascade_signals_the_lfg_types_twice_between_its_neighbours() {
    let mut c = client(
        r#"Seen = {}
        local f = CreateFrame("Frame")
        for _, e in ipairs({"PLAYER_ENTERING_WORLD", "QUEST_LOG_UPDATE",
                "UPDATE_LFG_TYPES", "SKILL_LINES_CHANGED"}) do
            f:RegisterEvent(e)
        end
        f:SetScript("OnEvent", function() table.insert(Seen, event) end)"#,
    );
    c.frame(vec![vec![me()]]);
    assert_eq!(
        c.seen(),
        [
            "PLAYER_ENTERING_WORLD",
            "QUEST_LOG_UPDATE",
            "UPDATE_LFG_TYPES",
            "UPDATE_LFG_TYPES",
            "SKILL_LINES_CHANGED",
        ]
    );
}

/// A player.
const ALICE: u64 = 0x31;
/// Another, listed with [`ALICE`].
const BOB: u64 = 0x32;

/// The display latch tests a line after its sender's name (`0x49d77f` before `0x49db5c`): an
/// uncached sender's name is asked at the packet, and the held line does not age, so however long
/// the loading screen lasts it shows at the cascade's release with its name.
#[test]
fn a_latched_line_asks_its_name_at_once_and_keeps_it_to_the_release() {
    let mut c = client(
        r#"Seen = {}
        local f = CreateFrame("Frame")
        for _, e in ipairs({"PLAYER_ENTERING_WORLD", "CHAT_MSG_SAY"}) do f:RegisterEvent(e) end
        f:SetScript("OnEvent", function()
            table.insert(Seen, event .. ":" .. tostring(arg1) .. ":" .. tostring(arg2))
        end)"#,
    );
    c.frame(vec![vec![SessionEvent::Chat(ChatMessage {
        chat_type: benilla_protocol::messages::CHAT_MSG_SAY,
        language: 0,
        sender_guid: ALICE,
        target_guid: 0,
        sender_name: None,
        channel: None,
        text: "hi".into(),
        chat_tag: 0,
    })]]);
    assert_eq!(c.name_queries(), [ALICE], "the name is asked at the packet");
    c.frame(vec![vec![named(ALICE, "Alice")]]);
    // A long loading screen: far past the name wait's frame budget.
    for _ in 0..200 {
        c.frame(vec![]);
    }
    assert_eq!(c.seen(), Vec::<String>::new(), "held under the latch");
    c.frame(vec![vec![me()]]);
    assert_eq!(
        c.seen(),
        ["PLAYER_ENTERING_WORLD:nil:nil", "CHAT_MSG_SAY:hi:Alice"]
    );
}

/// The latch holds a defence broadcast (`0x49de30`) as it holds a chat line, and a text emote
/// from a named performer apart, never to be shown (`0x49dc7f` → `0x49dc95`).
#[test]
fn the_latch_holds_a_defence_line_and_drops_a_named_emote() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let emotes = benilla_formats::load_emote_text_catalog(&mut chain).expect("emote texts");
    let mut c = client(
        r#"Seen = {}
        local f = CreateFrame("Frame")
        for _, e in ipairs({"PLAYER_ENTERING_WORLD", "CHAT_MSG_CHANNEL", "CHAT_MSG_TEXT_EMOTE"}) do
            f:RegisterEvent(e)
        end
        f:SetScript("OnEvent", function() table.insert(Seen, event .. ":" .. tostring(arg1)) end)"#,
    );
    c.app.insert_resource(crate::ui_chat::EmoteTexts(emotes));
    let world_defense =
        benilla_formats::ChatChannelsCatalog::from_rows(vec![benilla_formats::ChatChannelRow {
            id: 23,
            flags: 0x1_0004,
            pattern: "WorldDefense".into(),
            shortcut: "WorldDefense".into(),
        }]);
    c.app.insert_resource(crate::ui_chat::ChannelState {
        joined: vec![crate::ui_chat::edit::ChannelSlot::confirmed("WorldDefense", 23).into()],
        channels: world_defense,
        ..Default::default()
    });
    c.frame(vec![vec![named(ALICE, "Alice"), named(ME, "Me")]]);
    c.frame(vec![
        vec![SessionEvent::DefenseMessage {
            zone_id: 40,
            text: "under attack".into(),
        }],
        vec![SessionEvent::TextEmote {
            guid: ALICE,
            text_emote: 101,
            target_name: String::new(),
        }],
    ]);
    assert_eq!(c.seen(), Vec::<String>::new(), "both held under the latch");
    c.frame(vec![vec![me()]]);
    c.frame(vec![]);
    assert_eq!(
        c.seen(),
        ["PLAYER_ENTERING_WORLD:nil", "CHAT_MSG_CHANNEL:under attack"],
        "the defence line at the release; the emote never"
    );
}

/// A new VM's first frame signals each group list at its packet: the reference's interface is up
/// before the login goes out (`UI_Init` from `0x46c236`, the send at `0x46c272`), so no list lands
/// ahead of it, and the list handler signals inside itself (`0x5e6c66`) before the next packet runs
/// (`0x537ae5`). So after the arrival stamped at our create, each of two lists signals its own
/// `PARTY_MEMBERS_CHANGED`, ahead of a later packet's line.
#[test]
fn a_new_vms_first_lists_signal_at_their_packets() {
    use benilla_protocol::messages::{member_status, GroupMemberEntry};
    let mut c = client(
        r#"Seen = {}
        local f = CreateFrame("Frame")
        for _, e in ipairs({"PLAYER_ENTERING_WORLD", "PARTY_MEMBERS_CHANGED", "CHAT_MSG_SYSTEM"}) do
            f:RegisterEvent(e)
        end
        f:SetScript("OnEvent", function()
            table.insert(Seen, event .. ":" .. GetNumPartyMembers())
        end)"#,
    );
    let list = |guids: &[u64]| SessionEvent::GroupList {
        group_type: 0,
        own_flags: 0,
        members: guids
            .iter()
            .map(|&guid| GroupMemberEntry {
                name: format!("M{guid}"),
                guid,
                status: member_status::ONLINE,
                flags: 0,
            })
            .collect(),
        leader: ME,
        loot: None,
    };
    {
        let mut names = c.app.world_mut().resource_mut::<crate::names::NameCache>();
        names.insert_player(ALICE, "Alice".into(), None);
        names.insert_player(BOB, "Bob".into(), None);
    }
    c.frame(vec![
        vec![me()],
        vec![list(&[ALICE])],
        vec![list(&[ALICE, BOB])],
        vec![system_line("after")],
    ]);
    assert_eq!(
        c.seen(),
        [
            "PLAYER_ENTERING_WORLD:0",
            "PARTY_MEMBERS_CHANGED:1",
            "PARTY_MEMBERS_CHANGED:2",
            "CHAT_MSG_SYSTEM:2"
        ]
    );
}

/// A form learned after our create in its frame joins the list our arrival's replay rebuilds:
/// the reference's arrival ran inside the create's packet, ahead of the learn.
#[test]
fn a_form_learned_after_the_create_outlives_the_replay() {
    let mut c = client("Seen = {}");
    c.frame(vec![
        vec![SessionEvent::SpellBook {
            spell_ids: vec![BEAR],
            cooldowns: Vec::new(),
        }],
        vec![me()],
        vec![SessionEvent::SpellLearned { spell_id: CAT }],
    ]);
    let forms = c
        .app
        .world()
        .resource::<crate::ui_action::PlayerActions>()
        .forms
        .clone();
    assert_eq!(forms, [BEAR, CAT]);
}

/// The cascade's sound bracket opens before its `SelectTarget(0)` (`0x4908d5`, `0x490927`): a
/// target's lost-target sound at a world entry is dropped.
#[test]
fn the_world_entrys_target_clear_plays_no_sound() {
    let mut c = client(
        r#"Seen = {}
        local f = CreateFrame("Frame")
        f:RegisterEvent("PLAYER_TARGET_CHANGED")
        f:SetScript("OnEvent", function()
            table.insert(Seen, event .. ":" .. tostring(UnitExists("target")))
            if not UnitExists("target") then PlaySound("INTERFACESOUND_LOSTTARGETUNIT") end
        end)"#,
    );
    c.frame(vec![vec![me()]]);
    crate::script_calls::with_hosted_vm(c.app.world_mut(), |s| {
        s.expect("a VM").run("TargetUnit('player')").expect("runs")
    });
    c.frame(vec![]);
    assert_eq!(c.seen(), ["PLAYER_TARGET_CHANGED:1"], "targeted");
    world_sounds(&mut c);
    c.app
        .world_mut()
        .resource_mut::<crate::ui_world_enter::ArrivalLane>()
        .push(crate::ui_world_enter::Entry::Reload);
    // The lane alone, so the frame's sound drain leaves what the cascade queued.
    crate::ui_world_enter::fire_lane(c.app.world_mut());
    assert_eq!(
        c.seen(),
        ["PLAYER_TARGET_CHANGED:nil"],
        "the cascade's clear"
    );
    assert_eq!(
        world_sounds(&mut c),
        Vec::<String>::new(),
        "inside the bracket"
    );
}

/// The kit names the VM queued since the last call.
fn world_sounds(c: &mut Client) -> Vec<String> {
    c.app
        .world_mut()
        .non_send_resource_mut::<UiScript>()
        .take_sounds()
        .into_iter()
        .filter_map(|r| match r {
            benilla_ui::script::SoundRequest::KitName(n) => Some(n),
            _ => None,
        })
        .collect()
}

/// The cascade's silent resets (`0x4909ab`, `0x4909b0`, `0x4909b5`, `0x490a0a`): what the
/// windows' closes leave, the trainer's services, the vendor's rows and the stable's slots, goes
/// at their cascade place, after its `PLAYER_ENTERING_WORLD`, whose listener still reads what the
/// stable's close left (`0x4cae10` keeps the count, `0x4caab0` zeroes it).
#[test]
fn the_cascade_resets_what_the_closes_leave() {
    let mut c = client(
        r#"Seen = {}
        local f = CreateFrame("Frame")
        f:RegisterEvent("PLAYER_ENTERING_WORLD")
        f:SetScript("OnEvent", function()
            table.insert(Seen, event .. ":" .. GetNumStableSlots())
        end)"#,
    );
    let world = c.app.world_mut();
    world
        .resource_mut::<crate::ui_trainer::TrainerOpen>()
        .services = vec![benilla_protocol::messages::TrainerSpell {
        spell: 133,
        state: 0,
        cost: 0,
        can_learn_primary_prof: false,
        is_primary_prof_first_rank: false,
        req_level: 0,
        req_skill: 0,
        req_skill_value: 0,
        req_spells: [0; 3],
    }];
    world
        .resource_mut::<crate::ui_merchant::MerchantOpen>()
        .items = vec![benilla_protocol::messages::VendorItem {
        slot: 1,
        entry: 159,
        display_id: 0,
        current_count: 1,
        price: 1,
        max_durability: 0,
        buy_count: 1,
    }];
    world
        .resource_mut::<crate::ui_stable::StableOpen>()
        .num_stable_slots = 2;
    c.frame(vec![vec![me()]]);
    assert_eq!(
        c.seen(),
        ["PLAYER_ENTERING_WORLD:2"],
        "read before the reset"
    );
    let world = c.app.world();
    assert!(world
        .resource::<crate::ui_trainer::TrainerOpen>()
        .services
        .is_empty());
    assert!(world
        .resource::<crate::ui_merchant::MerchantOpen>()
        .items
        .is_empty());
    assert_eq!(
        world
            .resource::<crate::ui_stable::StableOpen>()
            .num_stable_slots,
        0
    );
}

/// `QueryCorpse` is the cascade's (`0x490a28` → `0x491f50`), after its time, mail, battlefield and
/// stone sends and ahead of the dead body's release: a ghost's `MSG_CORPSE_QUERY` goes out there.
#[test]
fn a_ghosts_corpse_query_goes_out_in_its_cascade_place() {
    /// `PLAYER_FLAGS`, bit 4 the ghost.
    const PLAYER_FLAGS: u16 = 190;
    let mut c = client("Seen = {}");
    c.sent.try_iter().for_each(drop);
    c.frame(vec![vec![created(
        ME,
        EntityKind::Player,
        &[
            (FIELD_UNIT_HEALTH, 1),
            (FIELD_UNIT_MAXHEALTH, 100),
            (PLAYER_FLAGS, 0x10),
        ],
    )]]);
    let order: Vec<String> = c
        .sent
        .try_iter()
        .map(|c| format!("{c:?}"))
        .filter(|c| {
            [
                "QueryTime",
                "QueryNextMailTime",
                "BattlefieldStatusRequest",
                "MeetingStoneStatusQuery",
                "CorpseQuery",
            ]
            .contains(&c.as_str())
        })
        .collect();
    assert_eq!(
        order,
        [
            "QueryTime",
            "QueryNextMailTime",
            "BattlefieldStatusRequest",
            "MeetingStoneStatusQuery",
            "CorpseQuery",
        ]
    );
}

/// `SMSG_NEW_WORLD` posts the object teardown as a timer at no delay (`0x401bac`), argument 1,
/// and the teardown's leave-world sweep sets the display latch (`0x467700` → `0x5dd543` →
/// `0x490a9e`). A tick runs its timers before its packets, so the lines of the worldport's own
/// tick show, those of the next are held to the next world entry's release, after its
/// `PLAYER_ENTERING_WORLD`.
#[test]
fn a_worldport_latches_the_lines_of_the_next_frame() {
    let mut c = client(
        r#"Seen = {}
        local f = CreateFrame("Frame")
        for _, e in ipairs({"PLAYER_ENTERING_WORLD", "CHAT_MSG_SYSTEM"}) do f:RegisterEvent(e) end
        f:SetScript("OnEvent", function() table.insert(Seen, event .. ":" .. tostring(arg1)) end)"#,
    );
    let system = |text: &str| {
        SessionEvent::Chat(ChatMessage {
            chat_type: CHAT_MSG_SYSTEM,
            language: 0,
            sender_guid: 0,
            target_guid: 0,
            sender_name: None,
            channel: None,
            text: text.into(),
            chat_tag: 0,
        })
    };
    c.frame(vec![vec![me()]]);
    assert_eq!(c.seen(), ["PLAYER_ENTERING_WORLD:nil"]);
    c.frame(vec![
        vec![system("before")],
        vec![SessionEvent::Worldport {
            map_id: 1,
            position: [0.0; 3],
            orientation: 0.0,
            needs_ack: true,
        }],
        vec![system("after")],
    ]);
    assert_eq!(
        c.seen(),
        ["CHAT_MSG_SYSTEM:before", "CHAT_MSG_SYSTEM:after"],
        "the port's own frame shows its lines"
    );
    c.frame(vec![vec![system("next")]]);
    assert_eq!(c.seen(), Vec::<String>::new(), "the next frame's line held");
    c.frame(vec![vec![me()]]);
    assert_eq!(
        c.seen(),
        ["PLAYER_ENTERING_WORLD:nil", "CHAT_MSG_SYSTEM:next"]
    );
}

/// A cross-map `SMSG_NEW_WORLD` in a frame, with `needs_ack` as the cross-map one carries it.
fn port() -> SessionEvent {
    SessionEvent::Worldport {
        map_id: 1,
        position: [0.0; 3],
        orientation: 0.0,
        needs_ack: true,
    }
}

/// A system line on the wire.
fn system_line(text: &str) -> SessionEvent {
    SessionEvent::Chat(ChatMessage {
        chat_type: CHAT_MSG_SYSTEM,
        language: 0,
        sender_guid: 0,
        target_guid: 0,
        sender_name: None,
        channel: None,
        text: text.into(),
        chat_tag: 0,
    })
}

const LEAVE_SPY: &str = r#"Seen = {}
    local f = CreateFrame("Frame")
    for _, e in ipairs({"PLAYER_ENTERING_WORLD", "CHAT_MSG_SYSTEM", "PLAYER_LEAVING_WORLD",
                        "LOGOUT_CANCEL"}) do
        f:RegisterEvent(e)
    end
    f:SetScript("OnEvent", function()
        local line = event .. ":" .. tostring(arg1)
        if event == "CHAT_MSG_SYSTEM" then line = line .. ":" .. tostring(UnitExists("player")) end
        table.insert(Seen, line)
    end)"#;

/// The teardown timer `SMSG_NEW_WORLD` posts at no delay (`0x401bac`) runs at the next tick's
/// start, ahead of its packets (`core/loop`, "What one tick runs"), and its body is one unit
/// (`0x401bc0`): the connection leaves the world (`0x401bd4`, `LOGOUT_CANCEL` for a pending
/// request), the objects are destroyed (`0x401bf7`) and the local player's destructor sweeps
/// (`0x5dd543` → `0x490a9e`), so `PLAYER_LEAVING_WORLD` follows every event the worldport's own
/// tick raised, the lines of the packets on either side of it among them, all of which still read
/// the old world and its pending logout.
#[test]
fn a_worldports_teardown_runs_at_the_next_ticks_start() {
    let mut c = client(LEAVE_SPY);
    c.frame(vec![vec![me()]]);
    assert_eq!(c.seen(), ["PLAYER_ENTERING_WORLD:nil"]);
    crate::script_calls::with_hosted_vm(c.app.world_mut(), |vm| {
        vm.expect("a VM").run("Logout()").expect("the verb runs")
    });
    c.frame(vec![
        vec![system_line("before")],
        vec![port()],
        vec![system_line("after")],
    ]);
    assert_eq!(
        c.seen(),
        ["CHAT_MSG_SYSTEM:before:1", "CHAT_MSG_SYSTEM:after:1"],
        "the port's own tick raised them, over the old world and its open logout"
    );
    assert!(
        c.app
            .world()
            .resource::<crate::ui_logout::LogoutState>()
            .pending(),
        "the request stands"
    );
    c.frame(vec![vec![system_line("next")]]);
    assert_eq!(
        c.seen(),
        ["LOGOUT_CANCEL:nil", "PLAYER_LEAVING_WORLD:nil"],
        "the next tick's timer runs first, and its line waits for the next world entry"
    );
    c.frame(vec![]);
    assert_eq!(c.seen(), Vec::<String>::new(), "once per port");
}

/// The leave-world sweep runs inside the object manager's walk at the local player's turn
/// (`0x467800` → `0x5dd543`), so an object created after the player is alive when
/// `PLAYER_LEAVING_WORLD` fires: the target still exists there, and is gone after the walk.
#[test]
fn a_worldports_leave_sees_the_target_created_after_the_player() {
    const NPC: u64 = 0xF130_0000_0000_0042;
    let mut c = client(
        r#"Seen = {}
        local f = CreateFrame("Frame")
        f:RegisterEvent("PLAYER_LEAVING_WORLD")
        f:SetScript("OnEvent", function()
            table.insert(Seen, event .. ":" .. tostring(UnitExists("target")))
        end)"#,
    );
    c.frame(vec![
        vec![me()],
        vec![created(
            NPC,
            EntityKind::Unit,
            &[(FIELD_UNIT_HEALTH, 50), (FIELD_UNIT_MAXHEALTH, 50)],
        )],
    ]);
    let world = c.app.world_mut();
    let npc = world.resource::<crate::net::GuidIndex>().0[&NPC];
    *world.resource_mut::<crate::target::Selection>() = crate::target::Selection {
        target: Some(npc),
        guid: Some(NPC),
        ..Default::default()
    };
    c.frame(vec![]);
    c.seen();
    c.frame(vec![vec![port()]]);
    assert_eq!(c.seen(), Vec::<String>::new());
    c.frame(vec![]);
    assert_eq!(c.seen(), ["PLAYER_LEAVING_WORLD:1"], "the target stands");
    assert!(
        !c.app
            .world()
            .resource::<crate::net::GuidIndex>()
            .0
            .contains_key(&NPC),
        "and goes with the walk"
    );
}

/// A worldport alone in its frame: nothing fires in it, the leave fires once in the next.
#[test]
fn a_worldport_alone_leaves_in_the_next_frame() {
    let mut c = client(LEAVE_SPY);
    c.frame(vec![vec![me()]]);
    assert_eq!(c.seen(), ["PLAYER_ENTERING_WORLD:nil"]);
    c.frame(vec![vec![port()]]);
    assert_eq!(c.seen(), Vec::<String>::new());
    c.frame(vec![]);
    assert_eq!(c.seen(), ["PLAYER_LEAVING_WORLD:nil"]);
    c.frame(vec![]);
    assert_eq!(c.seen(), Vec::<String>::new());
}

const ROLL_SPY: &str = r#"Seen = {}
    local f = CreateFrame("Frame")
    for _, e in ipairs({"START_LOOT_ROLL", "CANCEL_LOOT_ROLL"}) do f:RegisterEvent(e) end
    f:SetScript("OnEvent", function()
        table.insert(Seen, event .. ":" .. tostring(arg1))
    end)"#;

/// A group loot roll for an item the cache holds, so its frame opens in the frame it arrives.
fn roll_start() -> SessionEvent {
    const ROLLED: u32 = 19_019;
    SessionEvent::LootStartRoll(benilla_protocol::messages::LootStartRoll {
        looted_target: 0xF130_0000_0000_0042,
        item_slot: 0,
        item_id: ROLLED,
        random_property_id: 0,
        suffix_factor: 0,
        countdown_ms: 60_000,
    })
}

/// The client with a roll open for the spy's frame.
fn client_with_open_roll() -> Client {
    let mut c = client(ROLL_SPY);
    c.app
        .world_mut()
        .resource_mut::<crate::items::Items>()
        .insert_template(19_019, Some(crate::items::test_template("Sulfuras")));
    c.frame(vec![vec![me()]]);
    c.frame(vec![vec![roll_start()]]);
    assert_eq!(c.seen(), ["START_LOOT_ROLL:1"]);
    c
}

/// The world load's body calls `CancelAllLootRolls` (`0x401c2c`), after the objects' walk and
/// before the map load: each open roll signals `CANCEL_LOOT_ROLL` and is freed, in the tick the
/// timer runs, and `PLAYER_LEAVING_WORLD` (the walk's) came before it.
#[test]
fn a_worldport_cancels_the_open_loot_rolls_in_the_next_frame() {
    let mut c = client_with_open_roll();
    c.frame(vec![vec![port()]]);
    assert_eq!(
        c.seen(),
        Vec::<String>::new(),
        "the port's own tick cancels nothing"
    );
    assert!(c
        .app
        .world()
        .resource::<crate::ui_loot_roll::LootRolls>()
        .holds(1));
    c.frame(vec![]);
    assert_eq!(
        c.seen(),
        ["CANCEL_LOOT_ROLL:1"],
        "once, in the timer's tick"
    );
    assert!(!c
        .app
        .world()
        .resource::<crate::ui_loot_roll::LootRolls>()
        .holds(1));
    c.frame(vec![]);
    assert_eq!(c.seen(), Vec::<String>::new(), "once per port");
}

/// A teleport inside the map (`MSG_MOVE_TELEPORT`) runs no world load, so the roll stands.
#[test]
fn a_same_map_teleport_leaves_the_loot_rolls() {
    let mut c = client_with_open_roll();
    c.frame(vec![vec![SessionEvent::Teleport {
        guid: ME,
        counter: 1,
        position: [1.0, 2.0, 3.0],
        orientation: 0.0,
    }]]);
    c.frame(vec![]);
    assert_eq!(c.seen(), Vec::<String>::new());
    assert!(c
        .app
        .world()
        .resource::<crate::ui_loot_roll::LootRolls>()
        .holds(1));
}

/// The cascade's trainer reset zeroes the guid, the selection and the total (`0x4d73e0`), and not
/// the expanded mask (`0xb73a24`): the VM's collapsed lines stay for the next list to reset.
#[test]
fn the_trainer_reset_keeps_the_collapsed_lines() {
    let mut c = client("Seen = {}");
    c.frame(vec![vec![me()]]);
    {
        let world = c.app.world_mut();
        let mut s = world.non_send_resource_mut::<UiScript>();
        let service = |spell_id: u32, name: &str, group_key: u32, group: &str| {
            benilla_ui::script::TrainerService {
                spell_id,
                name: Some(name.into()),
                category: benilla_ui::script::TrainerServiceCategory::Available,
                group_key,
                group_name: group.into(),
                ..Default::default()
            }
        };
        s.set_trainer(Some(benilla_ui::script::TrainerState {
            guid: 0x42,
            services: vec![
                service(78, "Heroic Strike", 26, "Arms"),
                service(285, "Bloodrage", 256, "Fury"),
            ],
            ..Default::default()
        }));
        s.run("CollapseTrainerSkillLine(1) SelectTrainerService(2)")
            .unwrap();
    }
    let collapsed = c
        .app
        .world()
        .non_send_resource::<UiScript>()
        .trainer_collapsed_for_test();
    assert!(!collapsed.is_empty(), "a line collapsed");
    c.app
        .world_mut()
        .resource_mut::<crate::ui_world_enter::ArrivalLane>()
        .push(crate::ui_world_enter::Entry::Reload);
    c.frame(vec![]);
    let s = c.app.world().non_send_resource::<UiScript>();
    assert_eq!(s.trainer_collapsed_for_test(), collapsed, "the mask stays");
    assert_eq!(
        s.eval::<i64>("return GetTrainerSelectionIndex()").unwrap(),
        0,
        "the selection goes"
    );
}

/// A list after our create in the same frame outlives the cascade's reset, which the reference
/// ran inside the create's packet, ahead of the list's.
#[test]
fn a_list_after_the_create_outlives_the_cascades_reset() {
    let mut c = client("Seen = {}");
    c.app
        .world_mut()
        .resource_mut::<crate::ui_stable::StableOpen>()
        .num_stable_slots = 2;
    c.frame(vec![
        vec![me()],
        vec![SessionEvent::ListStabledPets {
            npc: 0x99,
            num_stable_slots: 1,
            pets: vec![],
        }],
    ]);
    assert_eq!(
        c.app
            .world()
            .resource::<crate::ui_stable::StableOpen>()
            .num_stable_slots,
        1
    );
}

/// The text emote's latch test runs once its performer's name is known (`0x49dc7f`), whatever
/// ours: a named performer's emote under the latch is dropped while our own name is unknown.
#[test]
fn a_latched_emote_drops_on_its_performers_name_alone() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let emotes = benilla_formats::load_emote_text_catalog(&mut chain).expect("emote texts");
    let mut c = client(
        r#"Seen = {}
        local f = CreateFrame("Frame")
        for _, e in ipairs({"PLAYER_ENTERING_WORLD", "CHAT_MSG_TEXT_EMOTE"}) do
            f:RegisterEvent(e)
        end
        f:SetScript("OnEvent", function() table.insert(Seen, event .. ":" .. tostring(arg1)) end)"#,
    );
    c.app.insert_resource(crate::ui_chat::EmoteTexts(emotes));
    c.frame(vec![vec![named(ALICE, "Alice")]]);
    c.frame(vec![vec![SessionEvent::TextEmote {
        guid: ALICE,
        text_emote: 101,
        target_name: String::new(),
    }]]);
    c.frame(vec![vec![me()]]);
    c.frame(vec![vec![named(ME, "Me")]]);
    c.frame(vec![]);
    assert_eq!(
        c.seen(),
        ["PLAYER_ENTERING_WORLD:nil"],
        "the emote never shows"
    );
}

/// The cascade's corpse step zeroes the range latch (`0x491f7b` → `0x4920f0(0)`): a set latch
/// signals `CORPSE_OUT_OF_RANGE` there, inside the arrival, after `PLAYER_ENTERING_WORLD`, once.
#[test]
fn the_cascades_corpse_step_signals_the_range_latch_out() {
    let mut c = client(
        r#"Seen = {}
        local f = CreateFrame("Frame")
        for _, e in ipairs({"PLAYER_ENTERING_WORLD", "CORPSE_OUT_OF_RANGE"}) do
            f:RegisterEvent(e)
        end
        f:SetScript("OnEvent", function() table.insert(Seen, event) end)"#,
    );
    c.app
        .world_mut()
        .resource_mut::<crate::death::DeathLatches>()
        .latch_corpse_in_range_for_test();
    c.frame(vec![vec![me()]]);
    c.frame(vec![]);
    assert_eq!(c.seen(), ["PLAYER_ENTERING_WORLD", "CORPSE_OUT_OF_RANGE"]);
}
