//! A joined channel's slot through its life: taken when the join is sent, confirmed by the server's
//! notice, freed after the line of a notice that drops it, and what the verbs that read it see.

use benilla_protocol::messages::channel_notice as n;
use benilla_ui::script::{ChannelCommand, UiScript};

use super::edit::{ChannelSlot, ChannelState, SlotRecord, SlotState};
use super::event::{ChatEvent, ChatEventKind};
use crate::net::{ClientCommand, NetCommands};

/// Rows 1, 2, 22, 23 and 25 as `ChatChannels.dbc` has them.
fn catalog() -> benilla_formats::ChatChannelsCatalog {
    benilla_formats::ChatChannelsCatalog::from_rows(
        [
            (1, 0x0_0003, "General - %s", "General"),
            (2, 0x0_003B, "Trade - %s", "Trade"),
            (22, 0x1_0003, "LocalDefense - %s", "LocalDefense"),
            (23, 0x1_0004, "WorldDefense", "WorldDefense"),
            (25, 0x2_0032, "GuildRecruitment - %s", "GuildRecruitment"),
        ]
        .into_iter()
        .map(
            |(id, flags, pattern, shortcut)| benilla_formats::ChatChannelRow {
                id,
                flags,
                pattern: pattern.into(),
                shortcut: shortcut.into(),
            },
        )
        .collect(),
    )
}

/// The verbs' world: the slots, a VM, the wire and the two sinks.
struct Verbs {
    channels: ChannelState,
    script: UiScript,
    commands: NetCommands,
    sent: crossbeam_channel::Receiver<ClientCommand>,
    log: super::ChatLog,
    errors: crate::ui_action::UiErrorKeys,
}

impl Verbs {
    fn new(joined: Vec<SlotRecord>) -> Self {
        let (tx, sent) = crossbeam_channel::unbounded();
        Verbs {
            channels: ChannelState {
                joined,
                channels: catalog(),
                zone_mask: Some(0),
                ..Default::default()
            },
            script: UiScript::new().expect("a VM"),
            commands: NetCommands(tx),
            sent,
            log: super::ChatLog::default(),
            errors: crate::ui_action::UiErrorKeys::default(),
        }
    }

    fn apply(&mut self, cmd: ChannelCommand) {
        super::input::apply_channel_command(
            cmd,
            &mut self.channels,
            &mut self.script,
            &self.commands,
            &self.log,
            &self.errors,
        );
    }

    fn sent(&self) -> Vec<ClientCommand> {
        self.sent.try_iter().collect()
    }

    fn error_keys(&self) -> Vec<&'static str> {
        self.errors.take().into_iter().map(|e| e.key).collect()
    }

    /// Count the `CHAT_MSG_CHANNEL_LIST` lines, keeping the last one's `arg1` and `arg4`.
    fn listen_for_lists(&self) {
        self.script
            .run(
                r#"
                local f = CreateFrame("Frame")
                f:RegisterEvent("CHAT_MSG_CHANNEL_LIST")
                f:SetScript("OnEvent", function()
                    LISTS = (LISTS or 0) + 1
                    LIST_TEXT, LIST_CHANNEL = arg1, arg4
                end)
            "#,
            )
            .expect("the handler installs");
    }

    /// What [`Self::listen_for_lists`] heard: the count, then the last line's `arg1` and `arg4`.
    fn lists(&self) -> (i64, String, String) {
        self.script
            .eval("return LISTS, LIST_TEXT, LIST_CHANNEL")
            .expect("a list line was heard")
    }

    /// One server notice through [`super::feed::deliver`], returned as it was rendered.
    fn notice(&mut self, byte: u8, channel: &str) -> ChatEvent {
        let mut event =
            super::feed::notice_event(byte, channel.into(), None, None).expect("a line");
        let mut windows = super::frames::ChatWindows::default();
        super::feed::deliver(
            &mut self.script,
            &mut windows,
            &mut self.channels,
            &mut event,
        );
        event
    }
}

fn custom(name: &str) -> SlotRecord {
    ChannelSlot::confirmed(name, 0).into()
}

fn join(name: &str, id: u32) -> ChannelCommand {
    ChannelCommand::Join {
        name: name.into(),
        password: String::new(),
        id,
    }
}

/// The join body registers the slot before it builds the packet (`0x49edf3`, then `0x49ee02`): the
/// allocator `0x49b980` fills it in state 1 (`0x49ba0f`) with the row's id (`0x49ba01`).
#[test]
fn a_join_takes_its_slot_when_it_is_sent() {
    let mut v = Verbs::new(vec![custom("World")]);
    v.apply(join("General - Elwynn Forest", 1));
    assert!(matches!(&v.sent()[..],
        [ClientCommand::JoinChannel { name, .. }] if name == "General - Elwynn Forest"));
    assert_eq!(v.channels.number_of("General - Elwynn Forest"), Some(2));
    assert_eq!(
        v.channels.slot_state("General - Elwynn Forest"),
        Some(SlotState::Pending),
        "no notice yet: the slot waits for the server"
    );
    assert_eq!(v.channels.joined[1].held().map(|s| s.id), Some(1));
}

/// The `YOU_JOINED` arm only confirms (`0x49c0c2` → `0x49bb20`): the first slot of the name not
/// yet in state 0 takes state 0 and sets its id's mask bit (`0x49bb8b`, `0x49bbaf`); a notice for a
/// name with no slot creates none and sets no bit.
#[test]
fn the_servers_join_notice_confirms_a_slot_and_creates_none() {
    let mut v = Verbs::new(Vec::new());
    v.apply(join("General - Elwynn Forest", 1));
    v.notice(n::YOU_JOINED, "General - Elwynn Forest");
    assert_eq!(
        v.channels.slot_state("General - Elwynn Forest"),
        Some(SlotState::Confirmed)
    );
    assert_eq!(v.channels.zone_mask, Some(0b1), "row 1's bit");

    v.notice(n::YOU_JOINED, "Trade - City");
    assert_eq!(
        v.channels.number_of("Trade - City"),
        None,
        "the join notice of a channel with no slot makes none"
    );
    assert_eq!(v.channels.zone_mask, Some(0b1), "and sets no bit");
}

/// With ten slots taken the allocator shows `0x199`, `ERR_TOO_MANY_CHAT_CHANNELS`
/// (`0x49b9c0`-`0x49b9ca`), and makes none; the join body ignores that (`0x49edf3`) and sends
/// (`0x49ee02`-`0x49ee42`).
#[test]
fn an_eleventh_join_shows_too_many_channels_and_is_still_sent() {
    let mut v = Verbs::new((1..=10).map(|i| custom(&format!("Chan{i}"))).collect());
    v.apply(join("Eleventh", 0));
    assert_eq!(v.error_keys(), ["ERR_TOO_MANY_CHAT_CHANNELS"]);
    assert!(matches!(&v.sent()[..],
        [ClientCommand::JoinChannel { name, .. }] if name == "Eleventh"));
    assert_eq!(v.channels.number_of("Eleventh"), None);

    v.apply(join("Chan3", 0));
    assert!(v.error_keys().is_empty(), "a held name is no new slot");
}

/// The notify handler sets its drop flag for `WRONG_PASSWORD` (`0x49c13c`), `NOT_MEMBER` and
/// `INVALID_NAME` (`0x49c46d`) and `WRONG_FACTION` (`0x49c446`), and after the line
/// (`0x49c5b0`) frees the slot of the notice's name (`0x49c5b5`-`0x49c5c2`). The line keeps the
/// number; the slot is no longer written to the chat cache (`0x499b90`-`0x499baa`).
#[test]
fn the_four_refusals_free_the_slot_after_their_line() {
    for byte in [
        n::WRONG_PASSWORD,
        n::NOT_MEMBER,
        n::WRONG_FACTION,
        n::INVALID_NAME,
    ] {
        let mut v = Verbs::new(vec![custom("World"), custom("MyChan")]);
        v.channels.custom = vec!["World".into(), "MyChan".into()];
        let line = v.notice(byte, "MyChan");
        assert_eq!(
            line.channel, "2. MyChan",
            "notice {byte}: the line is numbered"
        );
        assert_eq!(v.channels.number_of("MyChan"), None, "notice {byte}");
        assert_eq!(v.channels.number_of("World"), Some(1), "notice {byte}");
        assert_eq!(v.channels.custom, ["World"], "notice {byte}");
    }
    let mut v = Verbs::new(vec![custom("MyChan")]);
    v.notice(n::NOT_MODERATOR, "MyChan");
    assert_eq!(
        v.channels.number_of("MyChan"),
        Some(1),
        "any other refusal keeps it"
    );
}

/// `BANNED` (19) drops the slot as `WRONG_FACTION` does: its arm (`0x49c39a`) copies its token and
/// jumps into that arm's tail (`0x49c43a`), whose `0x49c446` sets the drop flag, so the slot frees
/// after the line (`0x49c5b5`-`0x49c5c2`) and a banned custom channel leaves the re-join list.
#[test]
fn a_banned_notice_frees_the_slot_after_its_line() {
    let mut v = Verbs::new(vec![custom("World")]);
    v.channels.custom = vec!["World".into(), "MyChan".into()];
    v.apply(join("MyChan", 0));
    let line = v.notice(n::BANNED, "MyChan");
    assert_eq!(
        line.channel, "2. MyChan",
        "the line reads the slot before it frees"
    );
    assert_eq!(v.channels.number_of("MyChan"), None);
    assert_eq!(
        v.channels.custom,
        ["World"],
        "not re-joined at the next login"
    );
    v.apply(join("Other", 0));
    assert_eq!(
        v.channels.number_of("Other"),
        Some(2),
        "the next join reuses it"
    );
}

/// A chat world whose host applies the channel verbs ([`super::run_chat`]), holding `channels`
/// and a VM with their mirror and the Elwynn Forest catalog.
fn hosted(channels: ChannelState) -> bevy::prelude::World {
    let mut script = UiScript::new().expect("a VM");
    script.set_zone_channel_catalog(super::channels::zone_channel_catalog(
        &channels.channels,
        "Elwynn Forest",
        0,
        None,
    ));
    channels.mirror_into(&mut script);
    let (mut world, _sent) = super::chat_world(script);
    world.insert_resource(channels);
    world
}

fn eval<T: benilla_ui::mlua::FromLuaMulti>(world: &bevy::prelude::World, lua: &str) -> T {
    world
        .non_send_resource::<UiScript>()
        .eval(lua)
        .expect("the chunk runs")
}

/// The join body takes its slot inside the call (`0x49ede8`-`0x49edf3`, the allocator
/// `0x49b980`) in state 1 (`0x49ba0f`): the name finds it on the verb's next line (`0x49be90`),
/// its number does not (`0x49bf30`). A held name keeps its slot (`0x49b991`); with all ten taken
/// the join gets none and shows `0x199` (`0x49b9c0`-`0x49b9ca`).
#[test]
fn a_join_takes_a_pending_slot_inside_the_call() {
    let mut channels = ChannelState {
        channels: catalog(),
        zone_mask: Some(0),
        ..Default::default()
    };
    channels.claim_slot("General - Elwynn Forest", 1);
    channels.confirm_slot("General - Elwynn Forest");
    channels.claim_slot("Gone", 0);
    channels.free_slot("Gone");
    let mut world = hosted(channels);
    super::run_chat(
        &mut world,
        "JoinChannelByName('MyChan') A = GetChannelName('MyChan') B = GetChannelName(2)",
    )
    .expect("the joins run");
    assert_eq!(
        eval::<(i64, i64)>(&world, "return A, B"),
        (2, 0),
        "the freed slot 2 is taken, pending"
    );
    super::run_chat(&mut world, "JoinChannelByName('General')").expect("the join runs");
    assert_eq!(
        eval::<i64>(&world, "return GetChannelName('General - Elwynn Forest')"),
        1,
        "a held name keeps its number"
    );
    for i in 3..=10 {
        super::run_chat(&mut world, &format!("JoinChannelByName('Chan{i}')")).expect("joins");
    }
    super::run_chat(&mut world, "JoinChannelByName('Eleventh')").expect("the join runs");
    assert_eq!(
        eval::<i64>(&world, "return GetChannelName('Eleventh')"),
        0,
        "ten slots taken: no slot"
    );
    let keys: Vec<&str> = world
        .resource::<crate::ui_action::UiErrorKeys>()
        .take()
        .into_iter()
        .map(|e| e.key)
        .collect();
    assert_eq!(keys, ["ERR_TOO_MANY_CHAT_CHANNELS"]);
}

/// The cascade's join goes through the join body (`0x49eb55` → `0x49eb70`): its slot is taken
/// before the cascade's `UPDATE_CHAT_WINDOWS` (`0x49eb5f`), in state 1 (`0x49ba0f`) until the
/// server confirms it, and the latch keeps AUTO, the cascade passing no reset flag.
#[test]
fn the_cascades_join_is_pending_until_the_server_confirms_it() {
    let mut channels = ChannelState {
        channels: catalog(),
        zone_mask: Some(0),
        ..Default::default()
    };
    channels.claim_slot("General - Orgrimmar", 1);
    channels.confirm_slot("General - Orgrimmar");
    let mut world = hosted(channels);
    {
        let mut script = world.non_send_resource_mut::<UiScript>();
        script.set_guild_recruitment_plan(benilla_ui::script::RecruitmentPlan::Join {
            name: "GuildRecruitment - City".into(),
            shortcut: "GuildRecruitment".into(),
            id: 25,
        });
        script
            .run(
                r#"
                local f = CreateFrame("Frame")
                f:RegisterEvent("UPDATE_CHAT_WINDOWS")
                f:SetScript("OnEvent", function()
                    SEEN = SEEN or GetChannelName("GuildRecruitment - City")
                end)
            "#,
            )
            .expect("the handler installs");
    }
    super::run_chat(&mut world, "SetGuildRecruitmentMode(1)").expect("the verb runs");
    assert_eq!(
        eval::<(i64, i64, i64)>(
            &world,
            "return SEEN, (GetChannelName('GuildRecruitment - City')), (GetChannelName(2))"
        ),
        (2, 2, 0)
    );
    assert_eq!(
        world
            .resource::<ChannelState>()
            .slot_state("GuildRecruitment - City"),
        Some(SlotState::Pending)
    );
    assert_eq!(
        world
            .non_send_resource::<UiScript>()
            .guild_recruitment_mode(),
        1
    );
}

/// The whole client headless, whose call host every entry that runs Lua lends, holding `held` as
/// slots 1.. (each confirmed but the last, which waits for its notice) and `script` as its VM.
struct Client {
    app: bevy::prelude::App,
    sent: crossbeam_channel::Receiver<ClientCommand>,
}

impl Client {
    fn new(held: &[&str], mut script: UiScript) -> Self {
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let (tx, sent) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        let mut channels = ChannelState {
            channels: catalog(),
            zone_mask: Some(0),
            ..Default::default()
        };
        for name in held {
            channels.claim_slot(name, channels.channels.zone_channel_id(name));
        }
        for name in &held[..held.len().saturating_sub(1)] {
            channels.confirm_slot(name);
        }
        channels.mirror_into(&mut script);
        app.insert_resource(channels);
        // What the startup builds, which the line drain reads.
        app.insert_resource(super::commands::SlashCommands::build(|_| None, |_| None));
        benilla_world::world_point::init_world_point_resources(app.world_mut());
        app.world_mut().insert_non_send_resource(script);
        Client { app, sent }
    }

    fn world(&mut self) -> &mut bevy::prelude::World {
        self.app.world_mut()
    }

    /// Run `lua` with the host lent, as every entry that runs Lua does.
    fn run(&mut self, lua: &str) {
        crate::script_calls::with_hosted_vm(self.world(), |s| s.expect("a VM").run(lua))
            .expect("the script runs");
    }

    fn number_of(&mut self, name: &str) -> Option<u32> {
        self.world().resource::<ChannelState>().number_of(name)
    }

    /// The names the `CMSG_JOIN_CHANNEL`s went out under, in order.
    fn joins(&self) -> Vec<String> {
        self.sent
            .try_iter()
            .filter_map(|c| match c {
                ClientCommand::JoinChannel { name, .. } => Some(name),
                _ => None,
            })
            .collect()
    }
}

/// A handler's join takes its slot inside its call (`0x49edf3`), against the slots as they stand
/// then: with 1-3 held, a `YOU_JOINED` handler's `JoinChannelByName("X")` takes 4 (`0x49b9b0`),
/// and the `YOU_LEFT` after it frees 2 and leaves X where it is, the notify handler freeing after
/// its event (`0x49c5b0`, `0x49c5b5`).
#[test]
fn a_handlers_join_keeps_the_slot_its_call_took() {
    let script = UiScript::new().expect("a VM");
    script
        .run(
            r#"
            local f = CreateFrame("Frame")
            f:RegisterEvent("CHAT_MSG_CHANNEL_NOTICE")
            f:SetScript("OnEvent", function()
                if arg1 == "YOU_JOINED" then JoinChannelByName("X") end
            end)
        "#,
        )
        .expect("the handler installs");
    let mut c = Client::new(&["A", "B", "C"], script);
    {
        let log = c.world().resource::<super::ChatLog>();
        let tail = benilla_protocol::messages::ChannelNoticeTail::Empty;
        log.push_channel_notice(n::YOU_JOINED, "C".into(), &tail);
        log.push_channel_notice(n::YOU_LEFT, "B".into(), &tail);
    }
    super::feed_chat_once(c.world());
    super::drain_chat_input_once(c.world());
    assert_eq!(c.number_of("X"), Some(4));
    assert_eq!(c.number_of("B"), None, "2 is free");
    assert_eq!(c.joins(), ["X"]);
}

/// `SetGuildRecruitmentMode(1)` runs the cascade inside the verb (`0x49ea7b` → `0x49eb55` →
/// `0x49eb70`), whose join takes its slot there (`0x49edf3`): a `/join` the same script makes after
/// it takes the next one, with 1-2 held 3 and then 4, each sent in that order.
#[test]
fn the_cascades_join_takes_its_slot_ahead_of_a_later_join() {
    let mut script = UiScript::new().expect("a VM");
    script.set_guild_recruitment_plan(benilla_ui::script::RecruitmentPlan::Join {
        name: "GuildRecruitment - City".into(),
        shortcut: "GuildRecruitment".into(),
        id: 25,
    });
    let mut c = Client::new(&["A", "B"], script);
    c.run("SetGuildRecruitmentMode(1) JoinChannelByName('Y')");
    super::drain_chat_input_once(c.world());
    bevy::ecs::system::RunSystemOnce::run_system_once(
        c.world(),
        super::recruitment::guild_recruitment_cascade,
    )
    .expect("the cascade runs");
    assert_eq!(c.number_of("GuildRecruitment - City"), Some(3));
    assert_eq!(c.number_of("Y"), Some(4));
    assert_eq!(c.joins(), ["GuildRecruitment - City", "Y"]);
}

/// Each chat verb sends inside its call, so call order is wire order: `DoEmote` (`0x49fd30`,
/// `0x5ef560` at `0x49fe16`), the join body (`0x49ee02`), `UninviteByName` (`0x48a669`) and
/// `SendAddonMessage` (`0x49faff`).
#[test]
fn the_chat_verbs_go_out_in_call_order() {
    let mut c = Client::new(&[], UiScript::new().expect("a VM"));
    // WAVE as a chat-only text emote, which reads `Emotes.dbc` row 0.
    c.world().insert_resource(crate::sound::EmoteSounds(
        benilla_formats::EmoteSoundCatalog::default()
            .with_text_emote("WAVE", 101, 0)
            .with_row(0, 0, 0),
    ));
    c.run(
        "DoEmote('WAVE') JoinChannelByName('X') UninviteByName('Bob') \
         SendAddonMessage('P', 'm') JoinChannelByName('Y')",
    );
    super::drain_chat_input_once(c.world());
    let wire: Vec<String> = c
        .sent
        .try_iter()
        .filter_map(|cmd| match cmd {
            ClientCommand::TextEmote { text_id, .. } => Some(format!("emote {text_id}")),
            ClientCommand::JoinChannel { name, .. } => Some(format!("join {name}")),
            ClientCommand::GroupUninvite { name } => Some(format!("uninvite {name}")),
            ClientCommand::AddonMessage { text, .. } => Some(format!("addon {text}")),
            _ => None,
        })
        .collect();
    assert_eq!(
        wire,
        [
            "emote 101",
            "join X",
            "uninvite Bob",
            "addon P\tm",
            "join Y"
        ]
    );
}

/// The leave body turns the auto-join off only when its key, a number read as its slot's name
/// (`0x49e880`), is the `GuildRecruitment` row's composed name (`SStrCmpI` at `0x49ef84`, then
/// `0x49ea70(0)` at `0x49ef8f`): `/leave GuildRecruitment` leaves the channel and keeps the
/// latch. The join's reset reads the row its Shortcut matched (`0x49ec0c`, `0x49ed3d`).
#[test]
fn only_the_composed_name_leaving_guild_recruitment_turns_its_auto_join_off() {
    let mut channels = ChannelState {
        channels: catalog(),
        zone_mask: Some(0),
        ..Default::default()
    };
    channels.claim_slot("GuildRecruitment - Elwynn Forest", 25);
    channels.confirm_slot("GuildRecruitment - Elwynn Forest");
    let mut world = hosted(channels);
    let after = |world: &mut bevy::prelude::World, lua: &str| {
        world
            .non_send_resource_mut::<UiScript>()
            .set_guild_recruitment_mode(1);
        super::run_chat(world, lua).expect("the verb runs");
        world
            .non_send_resource::<UiScript>()
            .guild_recruitment_mode()
    };
    assert_eq!(
        after(&mut world, "LeaveChannelByName('GuildRecruitment')"),
        1,
        "the Shortcut is not the composed name"
    );
    assert_eq!(
        after(
            &mut world,
            "LeaveChannelByName('guildrecruitment - elwynn forest')"
        ),
        0,
        "the composed name, case-folded"
    );
    assert_eq!(
        after(&mut world, "LeaveChannelByName(1)"),
        0,
        "slot 1's name"
    );
    assert_eq!(after(&mut world, "LeaveChannelByName('MyChan')"), 1);
    assert_eq!(
        after(&mut world, "JoinChannelByName('GuildRecruitment')"),
        0
    );
}

/// `ListChannels` adds its line inside the verb (`0x4a02c0` → `0x49a870`, type `0x11`): a
/// `CHAT_MSG_CHANNEL_LIST` handler has run by the verb's next line.
#[test]
fn list_channels_shows_its_line_inside_the_call() {
    let mut channels = ChannelState {
        channels: catalog(),
        zone_mask: Some(0),
        ..Default::default()
    };
    channels.claim_slot("MyChan", 0);
    channels.confirm_slot("MyChan");
    let mut world = hosted(channels);
    world
        .non_send_resource::<UiScript>()
        .run(
            r#"
            local f = CreateFrame("Frame")
            f:RegisterEvent("CHAT_MSG_CHANNEL_LIST")
            f:SetScript("OnEvent", function() SEEN = arg1 end)
        "#,
        )
        .expect("the handler installs");
    super::run_chat(&mut world, "ListChannels() AFTER = SEEN").expect("the verb runs");
    assert_eq!(
        eval::<Option<String>>(&world, "return AFTER").as_deref(),
        Some("[1. MyChan] ")
    );
}

/// The name a channel command went out under.
fn wire_name(c: &ClientCommand) -> &str {
    use ClientCommand as W;
    match c {
        W::ChannelList { name }
        | W::ChannelOwner { name }
        | W::ChannelModerate { name }
        | W::ChannelAnnouncements { name }
        | W::ChannelSetOwner { name, .. }
        | W::ChannelPassword { name, .. }
        | W::ChannelBan { name, .. }
        | W::ChannelInvite { name, .. }
        | W::ChannelKick { name, .. }
        | W::ChannelModerator { name, .. }
        | W::ChannelUnmoderator { name, .. }
        | W::ChannelMute { name, .. }
        | W::ChannelUnmute { name, .. }
        | W::ChannelUnban { name, .. } => name,
        other => panic!("not a channel command: {other:?}"),
    }
}

/// The fourteen admin verbs pass argument 1 through `0x49e880` (`0x4a014f`, `0x4a03df`,
/// `0x4a04f5`): a nonzero `SStrToInt` is the name of a slot in state 0 (`0x49be50`), and a number
/// naming none ends the call before the packet (`0x4a015a`, `0x4a03ea`, `0x4a0500`).
#[test]
fn the_admin_verbs_send_a_numbers_confirmed_slot_or_nothing() {
    let each = |key: &str| -> Vec<ChannelCommand> {
        let (name, player, password) = (key.to_string(), "Bob".to_string(), "pw".to_string());
        use ChannelCommand as C;
        vec![
            C::List { name: name.clone() },
            C::DisplayOwner { name: name.clone() },
            C::Moderate { name: name.clone() },
            C::ToggleAnnouncements { name: name.clone() },
            C::SetOwner {
                name: name.clone(),
                player: player.clone(),
            },
            C::SetPassword {
                name: name.clone(),
                password,
            },
            C::Ban {
                name: name.clone(),
                player: player.clone(),
            },
            C::Invite {
                name: name.clone(),
                player: player.clone(),
            },
            C::Kick {
                name: name.clone(),
                player: player.clone(),
            },
            C::Moderator {
                name: name.clone(),
                player: player.clone(),
            },
            C::Unmoderator {
                name: name.clone(),
                player: player.clone(),
            },
            C::Mute {
                name: name.clone(),
                player: player.clone(),
            },
            C::Unmute {
                name: name.clone(),
                player: player.clone(),
            },
            C::Unban { name, player },
        ]
    };
    let mut v = Verbs::new(vec![
        ChannelSlot::confirmed("General - Elwynn Forest", 1).into(),
        ChannelSlot::pending("MyChan", 0).into(),
    ]);
    for (key, wire) in [
        ("1", Some("General - Elwynn Forest")),
        ("2", None),
        ("9", None),
        ("MyChan", Some("MyChan")),
        ("0", Some("0")),
    ] {
        for cmd in each(key) {
            v.apply(cmd);
        }
        let sent = v.sent();
        match wire {
            Some(wire) => {
                assert_eq!(sent.len(), 14, "{key:?}");
                assert!(
                    sent.iter().all(|c| wire_name(c) == wire),
                    "{key:?}: {sent:?}"
                );
            }
            None => assert!(sent.is_empty(), "{key:?} names no confirmed slot: {sent:?}"),
        }
    }
}

/// A bare `/chatlist` is `ListChannels` (`0x4a01c0`): `"[%d. %s] "` (`0x844c94`) for each slot in
/// state 0 (`0x4a01f2`-`0x4a01fb`), posted as one `CHAT_MSG_CHANNEL_LIST` line with no channel
/// (`0x4a02a1`-`0x4a02c0`), empty or not.
#[test]
fn list_channels_posts_the_confirmed_slots_as_one_channel_list_line() {
    let mut v = Verbs::new(vec![
        ChannelSlot::confirmed("General - Elwynn Forest", 1).into(),
        ChannelSlot::pending("Trade - Elwynn Forest", 2).into(),
        ChannelSlot::confirmed("LocalDefense - Elwynn Forest", 22).into(),
        SlotRecord::Freed { id: 0 },
        ChannelSlot {
            state: SlotState::Renamed,
            ..ChannelSlot::confirmed("General - Westfall", 1)
        }
        .into(),
        custom("MyChan"),
    ]);
    v.listen_for_lists();
    v.apply(ChannelCommand::ListAll);
    assert_eq!(
        v.lists(),
        (
            1,
            "[1. General - Elwynn Forest] [3. LocalDefense - Elwynn Forest] [6. MyChan] ".into(),
            String::new()
        ),
        "one line, with no channel argument"
    );
    assert!(v.sent().is_empty());

    let mut v = Verbs::new(Vec::new());
    v.listen_for_lists();
    v.apply(ChannelCommand::ListAll);
    assert_eq!(
        v.lists(),
        (1, String::new(), String::new()),
        "an empty list still posts"
    );
}

/// The stock handler prints `arg1` alone for a channel list line with no channel
/// (`ChatFrame.lua:1408-1411`), and the log file the same.
#[test]
fn a_channel_list_line_with_no_channel_is_its_text_alone() {
    let mut event = ChatEvent::text_only(ChatEventKind::ChannelList, "[1. General] ".into());
    let get = |_: &str| Some("[%s] ".to_string());
    assert_eq!(
        super::frames::compose(&event, ChatEventKind::ChannelList, "", &get).as_deref(),
        Some("[1. General] ")
    );
    event.channel = "MyChan".into();
    assert_eq!(
        super::frames::compose(&event, ChatEventKind::ChannelList, "", &get).as_deref(),
        Some("[MyChan] [1. General] "),
        "a member list keeps its header"
    );
}

/// A line's arg7 is its slot's `+0x94` (`0x49b11e`): a custom channel whose name holds a row's
/// pattern is still id 0.
#[test]
fn a_channel_lines_id_is_its_slots_row() {
    let v = Verbs::new(vec![
        custom("WorldDefense2"),
        ChannelSlot::confirmed("General - Elwynn Forest", 1).into(),
    ]);
    let stamped = |name: &str| {
        let mut e = ChatEvent::text_only(ChatEventKind::Channel, "hi".into());
        e.channel = name.into();
        v.channels.stamp_channel(&mut e);
        (e.channel_number, e.zone_channel_id)
    };
    assert_eq!(stamped("WorldDefense2"), (1, 0));
    assert_eq!(stamped("General - Elwynn Forest"), (2, 1));
}
