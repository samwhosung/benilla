//! The chat verbs the host applies inside their calls ([`ChatSender`]): `SendChatMessage`
//! (`0x49f1e0`) past its binding's checks, from the group gates to the packet, the sends of
//! `RandomRoll` (`0x48c832`), `RequestTimePlayed` (`0x48a936`), `UninviteByName` (`0x48a669`) and
//! `SendAddonMessage` (`0x49faff`), and the channel verbs' slots and packets. No slash grammar runs
//! here: an addon sending `"/dance"` says six characters.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::{ChannelCommand, ChatSend, UiScript};

use super::super::away::{self, AfkMirror};
use super::super::edit::SendType;
use crate::net::{ChatKind, ClientCommand, NetCommands, ObjectStore};

/// What the chat verbs reach outside the VM, as the call host holds it.
#[derive(SystemParam)]
pub(crate) struct ChatSender<'w, 's> {
    commands: Res<'w, NetCommands>,
    chat_log: Res<'w, super::super::feed::ChatLog>,
    /// Where a refusal shows inside the call (`0x496720`).
    sink: crate::ui_action::MessageSink<'w>,
    /// The optimistic AFK mirror (`[0xb6e5cc]`) the AFK arm and the clear read and write; a world
    /// with no chat has none.
    mirror: Option<ResMut<'w, AfkMirror>>,
    objects: crate::net::Objects<'w, 's>,
    self_guid: Option<Res<'w, crate::net::SelfGuid>>,
    /// The selection `%t` names (`0xb4e2d8`).
    selection: Res<'w, crate::target::Selection>,
    names: Res<'w, crate::names::NameCache>,
    /// The channel slots the channel verbs read and the join's allocator writes; a world with no
    /// chat has none.
    channels: Option<ResMut<'w, super::super::edit::ChannelState>>,
    /// The `LoggingChat`/`LoggingCombat` files the verbs open inside their calls (`0x49fea2`); a
    /// world with no chat has none.
    logs: Option<Res<'w, super::super::logging::ChatLogFiles>>,
    /// The tutorial banks `AcknowledgeTutorial(0x16)` writes inside the call (`0x49f5fc`).
    tutorials: Option<Res<'w, crate::tutorial::Tutorials>>,
    /// The profanity masker the drunk transform asks (`0x508891`), and its `profanityFilter` gate.
    filter: Option<Res<'w, crate::text_filter::TextFilter>>,
    switches: Option<Res<'w, crate::text_filter::TextFilterSwitches>>,
}

/// [`ChatSender::refuse`] on the sink alone, for a verb that holds the sender's other state.
fn refuse_in(script: &mut UiScript, sink: &mut crate::ui_action::MessageSink, key: &'static str) {
    let input = script.input_events();
    if let Some(line) = crate::ui_action::keyed_line(script, key) {
        crate::ui_action::show_now(script, sink, "chat", line.under(input));
    }
}

impl ChatSender<'_, '_> {
    /// `DisplayError` (`0x496720`) inside the call, under its input events: a red line's
    /// `UI_ERROR_MESSAGE`, or a chat line's `CHAT_MSG_SYSTEM`, in place.
    fn refuse(&mut self, script: &mut UiScript, key: &'static str) {
        refuse_in(script, &mut self.sink, key);
    }

    /// A channel verb inside its call: a join's slot (`0x49ede8`-`0x49edf3`), the slots mirrored
    /// into the VM before the verb returns, and the packet it sends after them
    /// ([`Self::send_channel`]), with whether it showed a refusal ahead of it.
    pub(crate) fn channel(
        &mut self,
        script: &mut UiScript,
        cmd: ChannelCommand,
    ) -> Option<(ClientCommand, bool)> {
        let Some(channels) = self.channels.as_deref_mut() else {
            debug!("chat: no channel slots in this world; {cmd:?} dropped");
            return None;
        };
        let sink = &mut self.sink;
        let mut refused = false;
        let packet =
            super::channel::apply(cmd, channels, script, &self.chat_log, &mut |s, key| {
                refused = true;
                refuse_in(s, sink, key);
            })?;
        Some((packet, refused))
    }

    /// A channel verb's packet.
    pub(crate) fn send_channel(&self, packet: ClientCommand) {
        let _ = self.commands.0.send(packet);
    }

    /// A verb's own refusal, `DisplayError` (`0x496720`), shown inside its call.
    pub(crate) fn display_error(&mut self, script: &mut UiScript, key: &'static str) {
        self.refuse(script, key);
    }

    /// `MSG_RANDOM_ROLL` (`0x48c832`), the binding having checked the range.
    pub(crate) fn random_roll(&self, min: u32, max: u32) {
        let _ = self.commands.0.send(ClientCommand::RandomRoll { min, max });
    }

    /// `RequestTimePlayed`'s empty `CMSG_PLAYED_TIME` (`0x48a936`).
    pub(crate) fn request_time_played(&self) {
        let _ = self.commands.0.send(ClientCommand::PlayedTime);
    }

    /// `OpeningCinematic`'s empty `CMSG_OPENING_CINEMATIC` (`0x48c8f6`); the server answers, if
    /// at all, with the `SMSG_TRIGGER_CINEMATIC` [`crate::cinematic`] plays.
    pub(crate) fn opening_cinematic(&self) {
        let _ = self.commands.0.send(ClientCommand::OpeningCinematic);
    }

    /// `UninviteByName`'s `CMSG_GROUP_UNINVITE` (`0x48a669`).
    pub(crate) fn uninvite(&self, name: String) {
        let _ = self.commands.0.send(ClientCommand::GroupUninvite { name });
    }

    /// `LoggingChat(flag)` / `LoggingCombat(flag)` past the flag's write (`0x49fe69`): a flag now on
    /// with no log open opens it (`0x49fe79`-`0x49fea2`), and a failed open clears the flag
    /// (`0x49feab`) before the verb answers. A flag turned off closes the file.
    pub(crate) fn logging(&self, script: &mut UiScript, combat: bool) {
        let (chat, comb) = script.logging_flags();
        let on = if combat { comb } else { chat };
        let kept = match self.logs.as_deref() {
            Some(logs) => logs.apply(combat, on),
            None => false,
        };
        if on && !kept {
            script.set_logging(combat, false);
        }
    }

    /// `SendAddonMessage`'s send (`0x49faff`). The binding has applied the reference's rules
    /// (`0x49f920`: the four distributions, `prefix` TAB `message`, the downgrade outside a raid),
    /// so nothing is refused here. This writes the outbound half of the `addon` trace tag and
    /// `ui_chat::net` the inbound: `WOW_MOVE_TRACE=<path> WOW_MOVE_TRACE_TAGS=addon` logs both.
    pub(crate) fn addon_message(&self, send: benilla_ui::script::AddonSend) {
        debug!(
            "chat: addon broadcast on {} — {:?}",
            send.distribution.token(),
            send.text
        );
        if benilla_assets::trace::enabled() {
            benilla_assets::trace::line(
                "addon",
                &format!("-> {} {:?}", send.distribution.token(), send.text),
            );
        }
        let cmd = ClientCommand::AddonMessage {
            distribution: send.distribution,
            text: send.text,
        };
        if self.commands.0.send(cmd).is_err() {
            warn!("chat: not connected; addon broadcast dropped");
        }
    }

    /// `SendChatMessage` from its group gates on, in the reference's order: the group gates and
    /// the leader promotion, the channel slot, the AFK clear or the AFK arm, the DND arm, the dead
    /// check (`0x49f592`), the tutorial (`0x49f5fc`), the `%` expander (`0x49f60f`), the escape
    /// scan (`0x49f661`), and the packet (`0x49f6c3`). What a step does stands when a later step
    /// ends the call or raises.
    pub(crate) fn send_chat(&mut self, script: &mut UiScript, mut send: ChatSend) {
        // The binding has raised on any other token (`0x49f27f`), so none reaches here.
        let Some(mut kind) = SendType::from_token(&send.chat_type) else {
            return;
        };
        match group_gate(kind, send.group) {
            Ok(promoted) => kind = promoted,
            Err(key) => {
                self.refuse(script, key);
                return;
            }
        }
        // A channel target is a slot number, sent as that slot's name, read off the slot array
        // at the call; with no such slot the call ends here (`0x49f4ea`).
        if kind == SendType::Channel {
            let joined = self
                .channels
                .as_deref()
                .map(|c| c.names())
                .unwrap_or_default();
            let name = send
                .target
                .as_deref()
                .and_then(|t| benilla_ui::script::channel_send_target(&joined, t));
            if name.is_none() {
                debug!(
                    "chat: SendChatMessage names no joined channel {:?}; not sent",
                    send.target
                );
                return;
            }
            send.target = name;
        }
        let wire = kind.wire();
        // The AFK arm (`0x49f4f3`-`0x49f562`) ends the call with its own packet.
        if wire == ChatKind::Afk {
            self.afk(script, &send.text);
            return;
        }
        // Every other type first clears a standing AFK (`0x49f4f6 jne 0x49f3c7`), `/dnd` included.
        self.clear_afk(script);
        // The DND arm (`0x49f3de`-`0x49f58d`): its line, then the default text written back into
        // the message, which goes on through the steps below.
        if wire == ChatKind::Dnd {
            let strings = |key: &str| super::super::combat::global_string(script, key);
            let out = away::dnd_line(&send.text, self.player_flags() & 0x4 != 0, &strings);
            // The echo, type 0xa through `0x49a870` (`0x49f588`), fires inside the call.
            if let Some(line) = out.line {
                away::show_system(&self.chat_log, script, line);
            }
            send.text = out.body;
        }
        // A dead or ghost player's line is refused (`0x49f592`-`0x49f5e4`, `0x605f30`) unless its
        // type is on the allowed chain (`0x49f5a3`-`0x49f5da`).
        if !dead_may_send(kind) && self.player().is_some_and(|s| s.0.is_dead_or_ghost()) {
            self.refuse(script, "ERR_CHAT_WHILE_DEAD");
            return;
        }
        // `AcknowledgeTutorial(0x16)` (`0x49f5fc`), for every type that gets past the dead check,
        // the living's say, yell and emote included: its `CMSG_TUTORIAL_FLAG` goes out inside the
        // call, ahead of the line's packet (`0x49f72a`).
        if let Some(tutorials) = self.tutorials.as_deref() {
            tutorials.acknowledge_now(crate::tutorial::id::CHATTING, &self.commands);
        }
        let mut text =
            benilla_ui::script::expand_target_tokens(&send.text, || self.target_name(script));
        // A drunk player's text goes through `0x508680` (`0x49f614`-`0x49f656`), for every type
        // past the AFK arm, and the escape scan and the packet take its output (`0x49f65b`).
        let drunk = self
            .player()
            .and_then(|s| s.0.player_drunk_byte())
            .unwrap_or(0);
        if drunk != 0 {
            text = self.slur(script, &text, drunk);
        }
        if benilla_ui::script::validate_escapes(&mut text).is_err() {
            script.raise_in_call(benilla_ui::script::INVALID_ESCAPE);
            return;
        }
        // The gate `0x494a50(6)` (`0x49f6af`) passes every caller: tag 6 is its pass arm.
        let cmd = ClientCommand::Chat {
            kind: wire,
            target: send.target,
            text,
            language: send.language,
        };
        if self.commands.0.send(cmd).is_err() {
            warn!("chat: not connected; addon line dropped");
        }
    }

    /// `0x508680` over `text` for drunk byte `drunk` (`[[player+0xe68]+0x1d]`, `0x49f61a`): its
    /// strength is the byte cut to 100 and scaled by 0.01 (`0x49f62c`-`0x49f64e`), its buffer 256
    /// bytes (`0x49f651`). Deviation: a line of 256 bytes or more, which reaches here only through
    /// `chat_send`'s deviation for `0x49f604`, is slurred whole instead of cut to that buffer, so
    /// it reaches the server whole for the refusal that deviation leaves to it.
    fn slur(&self, script: &UiScript, text: &str, drunk: u8) -> String {
        let cap = if text.len() < 0x100 {
            0x100
        } else {
            usize::MAX
        };
        let enabled = self.switches.as_deref().is_none_or(|s| s.profanity);
        let slurred_speech =
            super::super::combat::global_string(script, "SLURRED_SPEECH").unwrap_or_default();
        super::drunk::slur(
            text,
            crate::player::drunk::fraction(drunk),
            cap,
            rand::random::<u32>,
            |probe| {
                self.filter
                    .as_deref()
                    .is_some_and(|f| f.mask(enabled, probe))
            },
            &slurred_speech,
        )
    }

    /// The AFK arm: `SetAFK` (`0x5eb740`) or the forced `ClearAFK(1)` (`0x5eb830`), each echoing
    /// and sending its own packet, language 0 (`0x5eb7ed`), not the argument's.
    fn afk(&mut self, script: &mut UiScript, msg: &str) {
        let strings = |key: &str| super::super::combat::global_string(script, key);
        let out = away::afk_line(msg, self.afk_mirror(), &strings);
        // `SetAFK`'s echo (`0x5eb7a9`) and the forced clear's (`0x5eb878`) fire inside the call.
        if let Some(line) = out.line {
            away::show_system(&self.chat_log, script, line);
        }
        if let (Some(v), Some(mirror)) = (out.mirror, self.mirror.as_mut()) {
            mirror.0 = v;
        }
        self.send_afk(out.body);
    }

    /// `ClearAFK(0)` (`0x5eb830`, from a send at `0x49f3d6` or a movement call's tick at
    /// `0x514e23`): with the mirror set and `autoClearAFK` on, the line (fired inside the call,
    /// `0x5eb878`), the mirror zeroed and the empty AFK packet, ahead of the message's own.
    pub(crate) fn clear_afk(&mut self, script: &mut UiScript) {
        if self.clear_afk_line(script) {
            self.finish_clear_afk();
        }
    }

    /// [`Self::clear_afk`] up to its line (`0x5eb878`), for a caller that runs the line's
    /// listeners there before [`Self::finish_clear_afk`]: whether it showed one.
    pub(crate) fn clear_afk_line(&mut self, script: &mut UiScript) -> bool {
        let strings = |key: &str| super::super::combat::global_string(script, key);
        let Some(line) = away::auto_clear_line(self.afk_mirror(), auto_clear_afk(script), &strings)
        else {
            return false;
        };
        away::show_system(&self.chat_log, script, line);
        true
    }

    /// [`Self::clear_afk`] past its line: the mirror zeroed (`0x5eb885`) and the empty AFK packet
    /// (`0x5eb8d0`).
    pub(crate) fn finish_clear_afk(&mut self) {
        if let Some(mirror) = self.mirror.as_mut() {
            mirror.0 = 0;
        }
        self.send_afk(String::new());
    }

    fn send_afk(&self, text: String) {
        let _ = self.commands.0.send(ClientCommand::Chat {
            kind: ChatKind::Afk,
            target: None,
            text,
            language: None,
        });
    }

    fn afk_mirror(&self) -> AfkMirror {
        self.mirror.as_deref().copied().unwrap_or_default()
    }

    /// Our own player's descriptor.
    fn player(&self) -> Option<&ObjectStore> {
        self.objects.store(self.self_guid.as_ref()?.0?)
    }

    /// Our live `PLAYER_FLAGS`: DND (`0x4`) has no mirror (`0x49f3f0`), so a second `/dnd` before
    /// the server's update marks again where a second `/afk` clears.
    fn player_flags(&self) -> u32 {
        self.player().map_or(0, |s| s.0.player_flags())
    }

    /// The name `0x5083a0` puts for a token (`0x50844a`-`0x5084bc`): with a selection, a unit the
    /// client holds is named by `GetUnitName` (`0x609210`), which asks its caches with a callback
    /// (`0x60924e`) and whose miss is `UNKNOWNOBJECT`; a guid it does not hold by the player-name
    /// cache with none (`0x508488`-`0x50849f`), which asks nothing on a miss (`0x55f125`); with
    /// neither, or no selection, `TARGET_TOKEN_NOT_FOUND`.
    fn target_name(&self, script: &UiScript) -> String {
        let not_found = || {
            super::super::combat::global_string(script, "TARGET_TOKEN_NOT_FOUND")
                .unwrap_or_default()
        };
        let Some(guid) = self.selection.guid.filter(|&g| g != 0) else {
            return not_found();
        };
        match self
            .objects
            .store(guid)
            .filter(|s| super::super::combat::is_unit(s))
        {
            Some(unit) => self
                .names
                .resolve_unit(
                    guid,
                    Some(unit),
                    crate::query_cache::Ask::New(&self.commands),
                )
                .map_or_else(|| script.unknown_object_text(), str::to_owned),
            None => self
                .names
                .resolve_player(guid, crate::query_cache::Ask::Read)
                .map_or_else(not_found, str::to_owned),
        }
    }
}

/// `autoClearAFK`, the record's integer (`SStrToInt` of its text), which `ClearAFK` tests
/// (`0x5eb84b`), read from the VM's table as the script left it; registered `"1"` (`0x5e24d4`). Off,
/// the clear is a total no-op: no echo, no mirror write, no packet.
fn auto_clear_afk(script: &UiScript) -> bool {
    script.cvar_int("autoClearAFK").is_none_or(|v| v != 0)
}

/// `SendChatMessage`'s group arms (`0x49f33e`-`0x49f4c7`): the refusal a type meets without its
/// group, as a message key (`DisplayError` `0x496720`), or the type that goes out. PARTY needs a
/// party member (id 0x48, `ERR_NOT_IN_GROUP`); RAID, RAID_WARNING and BATTLEGROUND need a raid
/// (0x19c, `ERR_NOT_IN_RAID`); RAID_WARNING also needs the leader or an assistant (0x4b,
/// `ERR_NOT_LEADER`). The group leader's RAID goes out as RAID_LEADER (`0x49f3bf`) and its
/// BATTLEGROUND as BATTLEGROUND_LEADER (`0x49f48b`).
fn group_gate(
    kind: SendType,
    group: benilla_ui::script::ChatGroup,
) -> Result<SendType, &'static str> {
    use SendType as T;
    match kind {
        T::Party if !group.in_party => Err("ERR_NOT_IN_GROUP"),
        T::Raid | T::RaidWarning | T::Battleground if group.raid_members == 0 => {
            Err("ERR_NOT_IN_RAID")
        }
        T::RaidWarning if !group.leads_or_assists => Err("ERR_NOT_LEADER"),
        T::Raid if group.leads => Ok(T::RaidLeader),
        T::Battleground if group.leads => Ok(T::BattlegroundLeader),
        other => Ok(other),
    }
}

/// Whether a dead or ghost player may send `kind`: the compare chain `0x49f5a3`-`0x49f5da` allows
/// whisper, party, raid, raid leader, raid warning, guild, officer, channel, AFK, DND, battleground
/// and battleground leader, so only say, yell and emote are refused (message 0xef,
/// `ERR_CHAT_WHILE_DEAD`).
fn dead_may_send(kind: SendType) -> bool {
    use SendType as T;
    match kind {
        T::Say | T::Yell | T::Emote => false,
        T::Whisper
        | T::Party
        | T::Raid
        | T::RaidLeader
        | T::RaidWarning
        | T::Guild
        | T::Officer
        | T::Channel
        | T::Afk
        | T::Dnd
        | T::Battleground
        | T::BattlegroundLeader => true,
    }
}

/// A world with what [`ChatSender`] reads, no AFK standing, nobody selected and no local player.
#[cfg(test)]
pub(crate) fn chat_world(script: UiScript) -> (World, crossbeam_channel::Receiver<ClientCommand>) {
    let mut world = World::new();
    world.insert_non_send_resource(script);
    let (tx, rx) = crossbeam_channel::unbounded();
    world.insert_resource(NetCommands(tx));
    world.init_resource::<super::super::feed::ChatLog>();
    world.init_resource::<super::super::logging::ChatLogFiles>();
    world.init_resource::<crate::sound::MessageSounds>();
    world.init_resource::<crate::ui_action::UiErrorKeys>();
    world.init_resource::<AfkMirror>();
    world.init_resource::<crate::net::GuidIndex>();
    world.init_resource::<crate::net::SelfGuid>();
    world.init_resource::<crate::target::Selection>();
    world.init_resource::<crate::names::NameCache>();
    world.init_resource::<super::super::edit::ChannelState>();
    world.init_resource::<crate::tutorial::Tutorials>();
    (world, rx)
}

/// Run `lua` on `world`'s VM with a host lent that applies the chat calls through [`ChatSender`]
/// off `world`, as the call host does inside each verb, and answers the group getters' questions
/// off `world`'s roster ([`crate::ui_party::RosterLive`], none in a world without one).
#[cfg(test)]
pub(crate) fn run_chat(world: &mut World, lua: &str) -> benilla_ui::mlua::Result<()> {
    let mut state =
        bevy::ecs::system::SystemState::<(ChatSender, crate::ui_party::RosterLive)>::new(world);
    let mut script = world.remove_non_send_resource::<UiScript>().expect("a VM");
    let r = {
        let (mut sender, roster) = state.get_mut(world);
        let apply = |script: &mut UiScript, call| match call {
            benilla_ui::script::ScriptCall::SendChat(send) => sender.send_chat(script, send),
            benilla_ui::script::ScriptCall::RandomRoll { min, max } => {
                sender.random_roll(min, max);
            }
            benilla_ui::script::ScriptCall::Channel(cmd) => {
                if let Some((packet, _)) = sender.channel(script, cmd) {
                    sender.send_channel(packet);
                }
            }
            benilla_ui::script::ScriptCall::Uninvite(name) => sender.uninvite(name),
            benilla_ui::script::ScriptCall::Logging { combat } => sender.logging(script, combat),
            benilla_ui::script::ScriptCall::RequestTimePlayed => sender.request_time_played(),
            benilla_ui::script::ScriptCall::AddonMessage(send) => sender.addon_message(send),
            other => panic!("not a chat call: {other:?}"),
        };
        let mut host = crate::script_calls::FnHostReading(apply, roster);
        script.hosted(&mut host, |s| s.run(lua))
    };
    state.apply(world);
    world.insert_non_send_resource(script);
    r
}
