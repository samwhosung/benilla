//! The account-data readers as a listener reads them: each runs inside the handler that settles
//! its section, the digests' check (`0x5afa10` → `0x5afe50` at `0x5afaca`, from `0x5af8e0`) or a
//! download (`0x5afc60` → `0x5afe50` at `0x5afd19`), so a listener of an earlier packet of the
//! frame reads the section as it stood, and one of a later packet reads what the reader read.

use benilla_protocol::field::FIELD_UNIT_HEALTH;
use benilla_protocol::messages::{AccountDataDigest, UpdateAccountData, ACCOUNT_DATA_SECTIONS};
use benilla_protocol::{EntityKind, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};

use super::unit_view_tests::{created, wire_packets};
use crate::local_state::test_env::{EnvGuard, ENV_LOCK};
use crate::net::ClientCommand;

const ME: u64 = 0x7;

/// The whole client, headless, in the world, over a state folder of its own.
struct Client {
    app: App,
    inbound: Sender<SessionEvent>,
    sent: Receiver<ClientCommand>,
    home: std::path::PathBuf,
    _env: (EnvGuard, EnvGuard),
}

impl Client {
    /// One frame of the client's, with `packets` in the inbound channel, each one packet.
    fn frame(&mut self, packets: Vec<Vec<SessionEvent>>) {
        for ev in wire_packets(packets) {
            self.inbound.send(ev).expect("the inbound channel");
        }
        self.app.update();
    }

    /// What the spy heard since the last call.
    fn seen(&mut self) -> Vec<String> {
        crate::script_calls::eval_in_vm(self.app.world(), "local s = Seen Seen = {} return s")
    }

    /// Our player's create, whose arrival loads the staged bar (`0x5deae2`), and a quiet frame.
    fn arrive(&mut self) {
        self.frame(vec![vec![created(
            ME,
            EntityKind::Player,
            &[(FIELD_UNIT_HEALTH, 100)],
        )]]);
        self.frame(vec![]);
    }

    /// The `CMSG_SET_ACTION_BUTTON`s sent since the last call, `(wire slot, word)`.
    fn placed(&mut self) -> Vec<(u8, u32)> {
        self.sent
            .try_iter()
            .filter_map(|c| match c {
                ClientCommand::SetActionButton { button, packed } => Some((button, packed)),
                _ => None,
            })
            .collect()
    }

    /// The bar's words, `(wire slot, word)` in slot order.
    fn bar(&self) -> Vec<(u8, u32)> {
        let slots = self.app.world().resource::<crate::ui_action::ActionSlots>();
        let mut words: Vec<(u8, u32)> = slots
            .buttons
            .values()
            .map(|b| (b.slot, (u32::from(b.kind) << 24) | b.action))
            .collect();
        words.sort_unstable();
        words
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.home).ok();
    }
}

/// The client in the world as `Probeone`, its interface loaded with every account-data reader
/// registered while its section is pending, as a world entry leaves them (`0x5af840`), our player
/// arrived, and a spy whose line for each of `events` carries its name and what `Probe()` answers
/// then. `files` are written first, each `(path, text)`.
fn client(
    name: &str,
    events: &[&str],
    probe: &str,
    files: impl FnOnce() -> Vec<(std::path::PathBuf, &'static str)>,
) -> Client {
    let mut c = unarrived(name, events, probe, files);
    c.arrive();
    c.seen();
    c
}

/// [`client`] before our player's create: the bar as the game entry leaves it, empty.
fn unarrived(
    name: &str,
    events: &[&str],
    probe: &str,
    files: impl FnOnce() -> Vec<(std::path::PathBuf, &'static str)>,
) -> Client {
    let home =
        std::env::temp_dir().join(format!("benilla-acct-live-{name}-{}", std::process::id()));
    std::fs::remove_dir_all(&home).ok();
    let env = (
        EnvGuard::unset("WOW_CAPTURE"),
        EnvGuard::set("BENILLA_HOME", home.to_str().unwrap()),
    );
    for (path, text) in files() {
        crate::local_state::write_atomic(&path, text).unwrap();
    }
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    app.insert_resource(bevy::ecs::error::DefaultErrorHandler(
        bevy::ecs::error::ignore,
    ));
    let (tx, sent) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    let (inbound, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetEvents::for_test(rx));
    app.insert_resource(crate::glue_session::roster::Roster::with_pending_pick(
        vec![crate::glue_session::roster::test_character(ME, "Probeone")],
        ME,
    ));
    app.update();
    assert!(
        app.world()
            .resource::<crate::account_data::AccountData>()
            .pending(crate::account_data::LAYOUT),
        "the sections pend until the server's digests"
    );
    let mut script = UiScript::new().expect("a VM");
    let events: Vec<String> = events.iter().map(|e| format!("{e:?}")).collect();
    script
        .run(&format!(
            r#"
            Seen = {{}}
            local f = CreateFrame("Frame")
            for _, e in ipairs({{{}}}) do
                f:RegisterEvent(e)
            end
            f:SetScript("OnEvent", function()
                table.insert(Seen, event .. ":" .. Probe())
            end)
            local p = CreateFrame("Frame", "LayoutProbe")
            p:SetWidth(100) p:SetHeight(100) p:SetPoint("BOTTOMLEFT", 0, 0)
            p:SetMovable(true) p:SetResizable(true)
            function Probe() {probe} end
            "#,
            events.join(", ")
        ))
        .unwrap();
    register_readers(app.world_mut(), &mut script);
    app.insert_non_send_resource(script);
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    app.world_mut().resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    Client {
        app,
        inbound,
        sent,
        home,
        _env: env,
    }
}

/// The interface load's registrations, as `CGGameUI::Initialize` makes them (`0x4900c2` the
/// layout's, `0x4900c7` → `0x4b62b0` the bindings', `0x4900d1` → `0x4f0a70` the macros').
fn register_readers(world: &mut World, script: &mut UiScript) {
    crate::bindings::seed_bindings_for_vm(script);
    crate::ui_layout::register_layout_reader(world, script);
    crate::bindings::register_binding_readers(world, script);
    crate::ui_macro::register_macro_readers(world, script);
}

/// `SMSG_ACCOUNT_DATA_MD5`: the server holds no copy of any section but those of `asked`, whose
/// copies differ from ours, so the check asks for each (`0x5afa6c`) and settles the rest.
fn digests(asked: &[usize]) -> SessionEvent {
    let mut server: [AccountDataDigest; ACCOUNT_DATA_SECTIONS] = Default::default();
    for &s in asked {
        server[s] = [0xab; 16];
    }
    SessionEvent::AccountDataMd5(server)
}

/// `SMSG_UPDATE_ACCOUNT_DATA` with `file` for `section`.
fn download(section: u32, file: &str) -> SessionEvent {
    let body = benilla_protocol::messages::update_account_data(section, file.as_bytes());
    SessionEvent::UpdateAccountData(UpdateAccountData {
        section,
        size: file.len() as u32,
        stream: body[8..].to_vec(),
    })
}

/// `SMSG_PLAYED_TIME`, which signals `TIME_PLAYED_MSG` inside itself (`0x4018df`).
fn played() -> SessionEvent {
    SessionEvent::PlayedTime {
        total: 60,
        level: 1,
    }
}

/// The character's binding section, downloaded between two packets of a frame, is read inside its
/// handler (`0x5afd19` → `0x4b6240` with set 2), whose set choice copies set 2 into the live set
/// and signals `UPDATE_BINDINGS` there (`0x4b629a` → `0x4b64b0` → `0x4b6310`, `0x4b6498`): the
/// earlier packet's listener reads the key unbound, the signal's and the later packet's read it.
#[test]
fn a_binding_download_is_read_inside_its_packet() {
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut c = client(
        "bindings",
        &["TIME_PLAYED_MSG", "UPDATE_BINDINGS"],
        r#"local key = GetBindingKey("BENILLAPROBE") return tostring(key)"#,
        Vec::new,
    );
    // The macro sections asked for too, so only the bindings' reader runs.
    let [_, character] = crate::account_data::BINDINGS;
    c.frame(vec![vec![digests(&[character, 4, 5])]]);
    c.seen();
    c.frame(vec![
        vec![played()],
        vec![download(3, "bind CTRL-F12 BENILLAPROBE\r\n")],
        vec![played()],
    ]);
    assert_eq!(
        c.seen(),
        [
            "TIME_PLAYED_MSG:nil",
            "UPDATE_BINDINGS:CTRL-F12",
            "TIME_PLAYED_MSG:CTRL-F12"
        ]
    );
}

/// The digests' check settles both macro sections in one packet, each in turn (`0x5af91b` →
/// `0x5afaca`), and each settling runs the reader for its scope (`0x4f0600`), which rebuilds the
/// order and signals `UPDATE_MACROS` (`0x4f0a50` → `0x4f0d30`): the earlier packet's listener reads
/// no macro, each signal's reads the scopes read so far, the later packet's reads both.
#[test]
fn the_macro_scopes_are_read_inside_the_digests_packet() {
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut c = client(
        "macros",
        &["TIME_PLAYED_MSG", "UPDATE_MACROS"],
        r#"local a, c = GetNumMacros() return a .. ":" .. c"#,
        || {
            vec![
                (
                    crate::local_state::macros_account_path().unwrap(),
                    "MACRO 1 \"Acct\" Ability_Ambush\n/say a\nEND\n",
                ),
                (
                    crate::local_state::macros_character_path("Realm", "Probeone").unwrap(),
                    "MACRO 16777217 \"Alt\" Ability_Ambush\n/say b\nEND\n",
                ),
            ]
        },
    );
    c.frame(vec![vec![played()], vec![digests(&[])], vec![played()]]);
    assert_eq!(
        c.seen(),
        [
            "TIME_PLAYED_MSG:0:0",
            "UPDATE_MACROS:1:0",
            "UPDATE_MACROS:1:1",
            "TIME_PLAYED_MSG:1:1"
        ]
    );
}

/// The layout section, downloaded between two packets of a frame, is read inside its handler
/// (`0x5afd19` → `0x490380`), which seats the window it names: the earlier packet's listener reads
/// the authored size, the later packet's the file's.
#[test]
fn a_layout_download_is_read_inside_its_packet() {
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut c = client(
        "layout",
        &["TIME_PLAYED_MSG"],
        r#"return math.floor(LayoutProbe:GetWidth() + 0.5)"#,
        Vec::new,
    );
    // The macro sections asked for too, so only the layout's reader runs.
    c.frame(vec![vec![digests(&[crate::account_data::LAYOUT, 4, 5])]]);
    c.seen();
    c.frame(vec![
        vec![played()],
        vec![download(6, "Frame: LayoutProbe\r\nW: 413\r\nH: 147\r\n")],
        vec![played()],
    ]);
    assert_eq!(c.seen(), ["TIME_PLAYED_MSG:100", "TIME_PLAYED_MSG:413"]);
}

/// A character file of the builds that numbered macros by position, ids 2, 1 and 3 in the order
/// they were made: those builds read it back by id, so Alpha (1) sat first, Bravo (2) second and
/// Charlie (3) third, on buttons `0x40000013` to `0x40000015`.
const OLD_CHARACTER_FILE: &str = "MACRO 2 \"Bravo\" Ability_Ambush\n/say b\nEND\n\
    MACRO 1 \"Alpha\" Ability_Ambush\n/say a\nEND\n\
    MACRO 3 \"Charlie\" Ability_Ambush\n/say c\nEND\n";

/// `SMSG_ACTION_BUTTONS` with three of those macros, one past the last (cleared), an account macro
/// and a word past 36, each `(wire slot, word)`.
fn old_bar() -> SessionEvent {
    let buttons = [
        (0u8, 0x4000_0013u32),
        (1, 0x4000_0015),
        (2, 0x4000_0014),
        (3, 0x4000_0016),
        (5, 0x4000_0001),
        (6, 0x4000_0025),
    ]
    .map(|(slot, word)| benilla_protocol::messages::ActionButton {
        slot,
        action: word & 0x00ff_ffff,
        kind: (word >> 24) as u8,
    });
    SessionEvent::ActionButtons {
        buttons: buttons.to_vec(),
    }
}

/// The moves of [`old_bar`]: Alpha, Charlie and Bravo by their ids, the button past the last
/// macro cleared, each once, as a placement goes.
const MOVED: [(u8, u32); 4] = [(0, 0x4100_0001), (1, 0x4100_0003), (2, 0x4100_0002), (3, 0)];

/// The bar after the moves: the account macro and the word past 36 stay.
const MOVED_BAR: [(u8, u32); 5] = [
    (0, 0x4100_0001),
    (1, 0x4100_0003),
    (2, 0x4100_0002),
    (5, 0x4000_0001),
    (6, 0x4000_0025),
];

/// The character's macro file on disk.
fn character_file() -> String {
    let path = crate::local_state::macros_character_path("Realm", "Probeone").unwrap();
    std::fs::read_to_string(path).unwrap()
}

/// At a login the macro sections settle ahead of the bar, which vmangos sends after the account
/// data (`CharacterHandler.cpp:525`, `:575`) and our player's arrival loads (`0x5deae2`): the
/// older numbering's moves wait for that load, and only once they have run is the file
/// renumbered.
#[test]
fn an_older_character_file_moves_its_buttons_once_the_bar_is_loaded() {
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut c = unarrived("renumber-bar-later", &[], "return ''", || {
        vec![(
            crate::local_state::macros_character_path("Realm", "Probeone").unwrap(),
            OLD_CHARACTER_FILE,
        )]
    });
    c.frame(vec![vec![digests(&[])]]);
    assert!(c.placed().is_empty(), "no bar yet: nothing moves");
    assert!(
        character_file().starts_with("MACRO 2 \"Bravo\""),
        "the file waits for the moves"
    );
    c.frame(vec![vec![old_bar()]]);
    assert!(c.placed().is_empty(), "the bar is only staged");
    c.arrive();
    assert_eq!(c.placed(), MOVED);
    assert_eq!(c.bar(), MOVED_BAR);
    assert!(
        character_file().starts_with("MACRO 16777217 \"Alpha\""),
        "renumbered, in name order: {}",
        character_file()
    );
    // The sections settle again: the renumbered file has no old ids, so nothing moves, though a
    // word from 19 to 36 would now be an account macro's.
    c.frame(vec![vec![digests(&[])]]);
    assert!(c.placed().is_empty(), "the buttons move once");
    assert_eq!(c.bar(), MOVED_BAR);
}

/// A server whose digests come after our player's arrival finds the bar loaded at the read: the
/// buttons move there, and the file is renumbered.
#[test]
fn an_older_character_file_moves_a_loaded_bar_at_its_read() {
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut c = unarrived("renumber-bar-first", &[], "return ''", || {
        vec![(
            crate::local_state::macros_character_path("Realm", "Probeone").unwrap(),
            OLD_CHARACTER_FILE,
        )]
    });
    c.frame(vec![vec![old_bar()]]);
    c.arrive();
    assert!(c.placed().is_empty(), "no macro read yet: nothing moves");
    c.frame(vec![vec![digests(&[])]]);
    assert_eq!(c.placed(), MOVED);
    assert_eq!(c.bar(), MOVED_BAR);
    assert!(
        character_file().starts_with("MACRO 16777217 \"Alpha\""),
        "renumbered, in name order: {}",
        character_file()
    );
}

/// An edit while the moves wait for the bar, a `CreateMacro` in a listener of the character
/// scope's `UPDATE_MACROS`, leaves the file in the older numbering, so a session that ends before
/// the arrival still finds the buttons' old places at its next load; the arrival moves them and
/// writes the table, the edit with it.
#[test]
fn an_edit_while_the_moves_wait_keeps_the_older_file_until_they_run() {
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut c = unarrived(
        "renumber-edit",
        &["UPDATE_MACROS"],
        r#"local _, c = GetNumMacros()
        if c > 0 and not Made then Made = 1 CreateMacro("Delta", 1, "/say d", nil, 1) end
        return ''"#,
        || {
            vec![(
                crate::local_state::macros_character_path("Realm", "Probeone").unwrap(),
                OLD_CHARACTER_FILE,
            )]
        },
    );
    c.frame(vec![vec![digests(&[])]]);
    c.frame(vec![]);
    assert!(
        crate::script_calls::eval_in_vm::<Option<u32>>(c.app.world(), "return Made").is_some(),
        "the listener made its macro"
    );
    assert_eq!(
        character_file(),
        OLD_CHARACTER_FILE,
        "the edit waits for the moves"
    );
    c.frame(vec![vec![old_bar()]]);
    c.arrive();
    assert_eq!(c.placed(), MOVED);
    let file = character_file();
    assert!(
        file.starts_with("MACRO 16777217 \"Alpha\"") && file.matches(" \"Delta\" ").count() == 1,
        "renumbered, the edit with it: {file}"
    );
}
