//! The chat verbs the host applies inside their calls ([`ChatSender`]): `SendChatMessage`
//! (`0x49f1e0`) past its binding's checks, from the group gates to the packet, and `RandomRoll`'s
//! send (`0x48c832`). No slash grammar runs here: an addon sending `"/dance"` says six characters.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::{ChatSend, UiScript};

use super::super::away::{self, AfkMirror};
use super::super::edit::SendType;
use crate::net::{ChatKind, ClientCommand, NetCommands, ObjectStore};

/// What the chat verbs reach outside the VM, as the call host holds it.
#[derive(SystemParam)]
pub(crate) struct ChatSender<'w, 's> {
    commands: Res<'w, NetCommands>,
    chat_log: Res<'w, super::super::feed::ChatLog>,
    errors: Res<'w, crate::ui_action::UiErrorKeys>,
    /// The optimistic AFK mirror (`[0xb6e5cc]`) the AFK arm and the clear read and write; a world
    /// with no chat has none.
    mirror: Option<ResMut<'w, AfkMirror>>,
    objects: crate::net::Objects<'w, 's>,
    self_guid: Option<Res<'w, crate::net::SelfGuid>>,
    /// The selection `%t` names (`0xb4e2d8`).
    selection: Res<'w, crate::target::Selection>,
    names: Res<'w, crate::names::NameCache>,
}

impl ChatSender<'_, '_> {
    /// `MSG_RANDOM_ROLL` (`0x48c832`), the binding having checked the range.
    pub(crate) fn random_roll(&self, min: u32, max: u32) {
        let _ = self.commands.0.send(ClientCommand::RandomRoll { min, max });
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
        match group_gate(kind, script.chat_group()) {
            Ok(promoted) => kind = promoted,
            Err(key) => {
                self.errors.push(crate::ui_action::UiError::key(key));
                return;
            }
        }
        // A channel target is a slot number, sent as that slot's name; with no such slot the call
        // ends here (`0x49f4ea`).
        if kind == SendType::Channel {
            let name = send
                .target
                .as_deref()
                .and_then(|t| script.channel_send_target(t));
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
            if let Some(line) = out.line {
                away::push_system(&self.chat_log, line);
            }
            send.text = out.body;
        }
        // A dead or ghost player's line is refused (`0x49f592`-`0x49f5e4`, `0x605f30`) unless its
        // type is on the allowed chain (`0x49f5a3`-`0x49f5da`).
        if !dead_may_send(kind) && self.player().is_some_and(|s| s.0.is_dead_or_ghost()) {
            self.errors
                .push(crate::ui_action::UiError::key("ERR_CHAT_WHILE_DEAD"));
            return;
        }
        // `AcknowledgeTutorial(0x16)` (`0x49f5fc`), for every type that gets past the dead check,
        // the living's say, yell and emote included.
        script.queue_tutorial_acknowledge(crate::tutorial::id::CHATTING);
        let mut text =
            benilla_ui::script::expand_target_tokens(&send.text, || self.target_name(script));
        // A drunk player's text goes through `0x508680` here (`0x49f614`-`0x49f656`), which
        // benilla does not build.
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

    /// The AFK arm: `SetAFK` (`0x5eb740`) or the forced `ClearAFK(1)` (`0x5eb830`), each echoing
    /// and sending its own packet, language 0 (`0x5eb7ed`), not the argument's.
    fn afk(&mut self, script: &UiScript, msg: &str) {
        let strings = |key: &str| super::super::combat::global_string(script, key);
        let out = away::afk_line(msg, self.afk_mirror(), &strings);
        if let Some(line) = out.line {
            away::push_system(&self.chat_log, line);
        }
        if let (Some(v), Some(mirror)) = (out.mirror, self.mirror.as_mut()) {
            mirror.0 = v;
        }
        self.send_afk(out.body);
    }

    /// `ClearAFK(0)` (`0x49f3d6`): with the mirror set and `autoClearAFK` on, the line, the mirror
    /// zeroed and the empty AFK packet, ahead of the message's own.
    fn clear_afk(&mut self, script: &UiScript) {
        let strings = |key: &str| super::super::combat::global_string(script, key);
        let Some(line) = away::auto_clear_line(self.afk_mirror(), auto_clear_afk(script), &strings)
        else {
            return;
        };
        away::push_system(&self.chat_log, line);
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
    world.init_resource::<crate::ui_action::UiErrorKeys>();
    world.init_resource::<AfkMirror>();
    world.init_resource::<crate::net::GuidIndex>();
    world.init_resource::<crate::net::SelfGuid>();
    world.init_resource::<crate::target::Selection>();
    world.init_resource::<crate::names::NameCache>();
    (world, rx)
}

/// Run `lua` on `world`'s VM with a host lent that applies the chat calls through [`ChatSender`]
/// off `world`, as the call host does inside each verb.
#[cfg(test)]
pub(crate) fn run_chat(world: &mut World, lua: &str) -> benilla_ui::mlua::Result<()> {
    let mut state = bevy::ecs::system::SystemState::<ChatSender>::new(world);
    let mut script = world.remove_non_send_resource::<UiScript>().expect("a VM");
    let r = {
        let mut sender = state.get_mut(world);
        let mut host = crate::script_calls::FnHost(|script: &mut UiScript, call| match call {
            benilla_ui::script::ScriptCall::SendChat(send) => sender.send_chat(script, send),
            benilla_ui::script::ScriptCall::RandomRoll { min, max } => {
                sender.random_roll(min, max);
            }
            other => panic!("not a chat call: {other:?}"),
        });
        script.hosted(&mut host, |s| s.run(lua))
    };
    state.apply(world);
    world.insert_non_send_resource(script);
    r
}
