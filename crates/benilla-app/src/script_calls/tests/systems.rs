//! Chunks that mix two systems' verbs through the whole client's host: each verb does its work
//! inside its call, as the reference's do, so the chunk's packets go out in the order its verbs
//! ran and its next line reads what the verb did.

use benilla_ui::mlua::FromLuaMulti;
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::Receiver;

use crate::net::{ClientCommand, NetCommands};
use crate::script_calls::with_hosted_vm;

/// The registrar and the tabard vendor, a guild master: a unit's guid.
pub(super) const NPC: u64 = 0xF130_0000_0000_0042;

/// The whole client, headless, with a VM and the write channel captured.
pub(super) fn client() -> (App, Receiver<ClientCommand>) {
    let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx));
    app.insert_non_send_resource(UiScript::new().expect("a VM"));
    (app, rx)
}

/// Run `chunk` with the host lent, as a key's body runs.
pub(super) fn eval<T: FromLuaMulti>(app: &mut App, chunk: &str) -> T {
    with_hosted_vm(app.world_mut(), |vm| vm.expect("a VM").eval::<T>(chunk))
        .unwrap_or_else(|e| panic!("{chunk}: {e}"))
}

/// What went out of the kinds these chunks' verbs send, in order.
fn sent(rx: &Receiver<ClientCommand>) -> Vec<String> {
    rx.try_iter()
        .filter_map(|c| {
            Some(match c {
                ClientCommand::RandomRoll { min, max } => format!("roll {min}-{max}"),
                ClientCommand::TutorialFlag { id } => format!("flag {id}"),
                ClientCommand::TutorialClear => "clear".into(),
                ClientCommand::TutorialReset => "reset".into(),
                ClientCommand::AddFriend { name } => format!("add friend {name}"),
                _ => return None,
            })
        })
        .collect()
}

/// Both tutorial banks as `SMSG_TUTORIAL_FLAGS` fills them (`0x4b5700`), and the VM told.
fn banks(app: &mut App, bank: [u8; 32]) {
    app.world()
        .resource::<crate::tutorial::Tutorials>()
        .lock()
        .apply_flags(&bank);
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .set_tutorial_bank(Some(bank.to_vec()));
}

/// `FlagTutorial(n)` acknowledges inside its call (`0x4b59dc` → `0x4b54c0`): its
/// `CMSG_TUTORIAL_FLAG` goes out ahead of the next verb's packet, and `TutorialsEnabled`
/// (`0x4b5960`) on the next line reads the bank the call wrote.
#[test]
fn flag_tutorial_sends_and_writes_the_bank_inside_its_call() {
    let (mut app, rx) = client();
    let mut bank = [0xFF; 32];
    bank[0] = !(1 << 4); // tutorial 5, 0-based 4, the one bit still clear
    banks(&mut app, bank);
    let (before, after): (Option<i64>, Option<i64>) = eval(
        &mut app,
        "local before = TutorialsEnabled() FlagTutorial(5) RandomRoll(1, 2) \
         return before, TutorialsEnabled()",
    );
    assert_eq!((before, after), (Some(1), None), "the bank the call wrote");
    assert_eq!(sent(&rx), ["flag 4", "roll 1-2"]);
}

/// `ClearTutorials()` (`0x4b5a00` → `0x4b55c0`) and `ResetTutorials()` (`0x4b5a10` → `0x4b5660`)
/// write both banks and send inside their calls.
#[test]
fn clear_and_reset_send_and_write_the_bank_inside_their_calls() {
    let (mut app, rx) = client();
    banks(&mut app, [0; 32]);
    let (cleared, reset): (Option<i64>, Option<i64>) = eval(
        &mut app,
        "ClearTutorials() local cleared = TutorialsEnabled() RandomRoll(1, 2) \
         ResetTutorials() return cleared, TutorialsEnabled()",
    );
    assert_eq!((cleared, reset), (None, Some(1)));
    assert_eq!(sent(&rx), ["clear", "roll 1-2", "reset"]);
}

/// `FriendList::AddFriend` acknowledges tutorial 21 through `0x4b54c0` (`0x5ae677`) before its
/// `CMSG_ADD_FRIEND` (`0x5ae6b9`), both inside the call; an acknowledged tutorial sends no second
/// flag (`0x4b54eb`).
#[test]
fn add_friend_flags_the_friends_tutorial_before_its_packet() {
    let (mut app, rx) = client();
    banks(&mut app, [0; 32]);
    eval::<()>(
        &mut app,
        r#"AddFriend("Bob") RandomRoll(1, 2) AddFriend("Ann")"#,
    );
    assert_eq!(
        sent(&rx),
        ["flag 21", "add friend Bob", "roll 1-2", "add friend Ann"]
    );
}

/// A frame counting `event`'s fires in `N` and recording each in `SEQ`, whose handler also calls
/// `close`, as a stock window's `OnHide` does.
pub(super) fn listen(app: &mut App, events: &[&str], close: &str) {
    let registers: String = events
        .iter()
        .map(|e| format!("f:RegisterEvent('{e}') "))
        .collect();
    eval::<()>(
        app,
        &format!(
            "N = 0 SEQ = '' local f = CreateFrame('Frame') {registers} \
             f:SetScript('OnEvent', function() N = N + 1 SEQ = SEQ .. event .. ' ' {close} end)"
        ),
    );
}

/// The registrar open on the latch, as `SMSG_PETITION_SHOWLIST`'s opener leaves it
/// (`0x4f4fc0`, `0x4f4fdc`).
fn open_registrar(app: &mut App) {
    let world = app.world_mut();
    crate::ui_session::register(world, NPC);
    world
        .resource_mut::<crate::ui_petition::GuildRegistrarState>()
        .open_for_test(NPC);
}

/// `CloseGuildRegistrar()` (`0x4f5220` → `0x4f5010`) releases the latch and signals
/// `GUILD_REGISTRAR_CLOSED` (`0x4f503a`) inside its call, twice while the registrar holds the
/// latch (`0x4f5024` → `0x4931c0` → `0x493310`, which re-enters the core); the handler's own close
/// finds the cell clear. The next line reads both.
#[test]
fn close_guild_registrar_signals_inside_its_call() {
    let (mut app, rx) = client();
    open_registrar(&mut app);
    listen(
        &mut app,
        &["GUILD_REGISTRAR_CLOSED"],
        "CloseGuildRegistrar()",
    );
    let n: i64 = eval(
        &mut app,
        "CloseGuildRegistrar() local n = N RandomRoll(1, 2) return n",
    );
    assert_eq!(n, 2);
    assert_eq!(sent(&rx), ["roll 1-2"]);
    assert_eq!(
        app.world()
            .resource::<crate::ui_session::InteractLatch>()
            .guid(),
        None,
        "the latch released inside the call"
    );
    assert_eq!(eval::<i64>(&mut app, "CloseGuildRegistrar() return N"), 2);
}

/// `GetTabardInfo()` runs the registrar's close core after its opener (`0x4f5355` → `0x4f5010`),
/// inside its call.
#[test]
fn get_tabard_info_closes_the_registrar_inside_its_call() {
    let (mut app, _rx) = client();
    open_registrar(&mut app);
    listen(
        &mut app,
        &["GUILD_REGISTRAR_CLOSED"],
        "CloseGuildRegistrar()",
    );
    assert_eq!(eval::<i64>(&mut app, "GetTabardInfo() return N"), 2);
}

/// `CloseTabardCreation()` (`0x4f5900` → `0x4f58a0`) releases the latch and signals
/// `CLOSE_TABARD_FRAME` (`0x4f58f8`) inside its call, twice while the designer holds the latch,
/// behind the open not yet told; the handler's own close finds the cell clear.
#[test]
fn close_tabard_creation_signals_inside_its_call() {
    let (mut app, rx) = client();
    crate::ui_tabard::open_tabard(app.world_mut(), NPC);
    listen(
        &mut app,
        &["OPEN_TABARD_FRAME", "CLOSE_TABARD_FRAME"],
        "if event == 'CLOSE_TABARD_FRAME' then CloseTabardCreation() end",
    );
    let seq: String = eval(
        &mut app,
        "CloseTabardCreation() local s = SEQ RandomRoll(1, 2) return s",
    );
    assert_eq!(
        seq,
        "OPEN_TABARD_FRAME CLOSE_TABARD_FRAME CLOSE_TABARD_FRAME "
    );
    assert_eq!(sent(&rx), ["roll 1-2"]);
    assert_eq!(
        app.world()
            .resource::<crate::ui_session::InteractLatch>()
            .guid(),
        None
    );
}
