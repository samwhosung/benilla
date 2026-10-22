//! `/chatlist`'s answer, `SMSG_CHANNEL_LIST` (`0x49c690`): the members wait on their channel's
//! slot until every name is known, then the printer `0x49c7e0` posts them by name as
//! `CHAT_MSG_CHANNEL_LIST` lines, which the stock `ChatFrame_OnEvent` prints after
//! `CHAT_CHANNEL_LIST_GET` (`ChatFrame.lua:1407-1412`).

use bevy::prelude::*;

use super::{ChatEvent, ChatEventKind, ChatLog};
use crate::names::NameCache;
use crate::net::NetCommands;
use crate::query_cache::Ask;

/// The members each channel slot holds for its list, by slot number: the slot's list
/// (`+0x88`) and the packet's flag byte (`+0x90`).
#[derive(Resource, Default)]
pub(crate) struct ChannelRosters(std::collections::BTreeMap<u32, Roster>);

impl ChannelRosters {
    /// Every list dropped with the slots that held them: the session's end.
    pub(super) fn clear(&mut self) {
        self.0.clear();
    }
}

#[derive(Default)]
struct Roster {
    /// The channel's flag byte; with `0x02` a member without `0x04` reads `#` (`0x49c8d2`).
    flags: u8,
    /// `(guid, member flags)`, appended in packet order (the tail link, `0x49c75d`-`0x49c775`).
    members: Vec<(u64, u8)>,
}

/// The handler `0x49c690`. A channel no slot holds by name drops the list (`0x49c6b8`-`0x49c6d6`).
/// The flag byte is stored on the slot (`0x49c6e2`), each member appended and its name asked with
/// the callback (`0x49c7a2`), and the printer runs when every name was already known
/// (`0x49c7c0`-`0x49c7c9`).
pub(super) fn receive(
    rosters: &mut ChannelRosters,
    number: Option<u32>,
    names: &NameCache,
    commands: &NetCommands,
    chat_log: &ChatLog,
    slot_name: &str,
    flags: u8,
    members: &[(u64, u8)],
) {
    let Some(number) = number else {
        debug!("chat: channel list for a channel no slot holds; dropped");
        return;
    };
    let roster = rosters.0.entry(number).or_default();
    roster.flags = flags;
    let mut missing = 0;
    for &member in members {
        roster.members.push(member);
        if names
            .resolve_player(member.0, Ask::Event(commands))
            .is_none()
        {
            missing += 1;
        }
    }
    if missing == 0 {
        print(rosters, number, names, chat_log, slot_name);
    }
}

/// The name callback's walk (`0x49cfeb`-`0x49d0b4`): on an answer, every slot holding a list drops
/// the members whose name the server refused (`0x49d03b`-`0x49d087`) and runs the printer.
pub(super) fn print_answered(
    rosters: &mut ChannelRosters,
    slot_name: impl Fn(u32) -> Option<String>,
    names: &NameCache,
    chat_log: &ChatLog,
) {
    let numbers: Vec<u32> = rosters.0.keys().copied().collect();
    for number in numbers {
        if let Some(roster) = rosters.0.get_mut(&number) {
            roster
                .members
                .retain(|&(guid, _)| !names.player_answered_unknown(guid));
            if roster.members.is_empty() {
                rosters.0.remove(&number);
                continue;
            }
        }
        match slot_name(number) {
            Some(name) => print(rosters, number, names, chat_log, &name),
            None => {
                rosters.0.remove(&number);
            }
        }
    }
}

/// The printer `0x49c7e0`: nothing while a member's name is unknown (`0x49c81d`-`0x49c85a`), else
/// the lines [`list_lines`] builds, each through `0x49a870` as type 0x11 with the slot's name
/// (`0x49c9dc`, `0x49ca7b`), and the list freed (`0x49ca82`-`0x49cad3`).
fn print(
    rosters: &mut ChannelRosters,
    number: u32,
    names: &NameCache,
    chat_log: &ChatLog,
    slot_name: &str,
) {
    let Some(roster) = rosters.0.get(&number) else {
        return;
    };
    let mut named = Vec::with_capacity(roster.members.len());
    for &(guid, member_flags) in &roster.members {
        let Some(name) = names.resolve_player(guid, Ask::Read) else {
            return;
        };
        named.push((name.to_string(), member_flags));
    }
    for line in list_lines(roster.flags, &named) {
        let mut event = ChatEvent::text_only(ChatEventKind::ChannelList, line);
        event.channel = slot_name.to_string();
        chat_log.push_event(event);
    }
    rosters.0.remove(&number);
}

/// The printer's text (`0x49c85c`-`0x49ca7b`): one `"%s%s%s"` entry per member (`0x8362d8`, cut at
/// 50 bytes by `SStrPrintf(buf, 0x33)`, `0x49c98f`), the separator `", "` unless the line is empty
/// (`0x49c8e5`-`0x49c8f6`), then `*` for the owner (member flag `0x01`), `@` for a moderator
/// (`0x02`) or `#` for a member without `0x04` when the channel's flags have `0x02`
/// (`0x49c8b2`-`0x49c8e0`), then the name. An entry that would bring the line to 256 bytes posts
/// the line first and starts the next without its separator (`0x49c9b0`-`0x49ca10`). The name
/// cache's realm, `-` and the realm after the name when set (`0x49c91b`-`0x49c975`), is not kept
/// here: a 1.12 server sends it empty.
pub(super) fn list_lines(channel_flags: u8, members: &[(String, u8)]) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut entries = 0;
    for (name, flags) in members {
        let prefix = if flags & 0x01 != 0 {
            "*"
        } else if flags & 0x02 != 0 {
            "@"
        } else if channel_flags & 0x02 != 0 && flags & 0x04 == 0 {
            "#"
        } else {
            ""
        };
        let separator = if line.is_empty() { "" } else { ", " };
        let mut entry = format!("{separator}{prefix}{name}");
        cut_at(&mut entry, 0x33 - 1);
        if entry.len() + line.len() >= 0x100 {
            lines.push(std::mem::take(&mut line));
            if entry.starts_with(',') {
                entry.drain(..2);
            }
            line.push_str(&entry);
            entries = 1;
        } else {
            line.push_str(&entry);
            entries += 1;
        }
    }
    if entries != 0 {
        lines.push(line);
    }
    lines
}

/// `text` cut to at most `max` bytes, backed off to a char boundary.
fn cut_at(text: &mut String, max: usize) {
    if text.len() > max {
        let mut cut = max;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
    }
}

/// Register the list's handler and the name answer's walk, and the lists they hold.
pub(super) fn register(app: &mut App) {
    use crate::net::NetHandlerApp;
    use benilla_protocol::SessionEventKind as K;
    app.init_resource::<ChannelRosters>()
        .net_handler(K::ChannelList, on_channel_list)
        .net_handler(K::PlayerName, on_player_name);
}

/// `SMSG_CHANNEL_LIST`'s handler: the slot found by its name in any state (`0x49be90`).
fn on_channel_list(
    In(ev): In<benilla_protocol::SessionEvent>,
    mut rosters: ResMut<ChannelRosters>,
    channels: Option<Res<super::ChannelState>>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    chat_log: Res<ChatLog>,
) {
    let benilla_protocol::SessionEvent::ChannelList {
        channel,
        flags,
        members,
    } = ev
    else {
        return;
    };
    let slot = channels.as_deref().and_then(|c| {
        let number = c.number_of(&channel)?;
        Some((number, slot_name(c, number)?))
    });
    receive(
        &mut rosters,
        slot.as_ref().map(|(n, _)| *n),
        &names,
        &commands,
        &chat_log,
        slot.as_ref().map_or("", |(_, name)| name.as_str()),
        flags,
        &members,
    );
}

/// The name callback's walk at a player-name answer, a refusal included, after the cache has
/// filed it (`0x55f426`-`0x55f452`), so the lines take the answer's place among the packets.
fn on_player_name(
    In(ev): In<benilla_protocol::SessionEvent>,
    mut rosters: ResMut<ChannelRosters>,
    channels: Option<Res<super::ChannelState>>,
    names: Res<NameCache>,
    chat_log: Res<ChatLog>,
) {
    if !matches!(ev, benilla_protocol::SessionEvent::PlayerName { .. }) || rosters.0.is_empty() {
        return;
    }
    let channels = channels.as_deref();
    print_answered(
        &mut rosters,
        |n| channels.and_then(|c| slot_name(c, n)),
        &names,
        &chat_log,
    );
}

/// Slot `number`'s name (`+0x04`), the channel the printer posts under.
fn slot_name(channels: &super::ChannelState, number: u32) -> Option<String> {
    channels
        .joined
        .get(number.checked_sub(1)? as usize)?
        .held()
        .map(|s| s.name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(members: &[(&str, u8)]) -> Vec<(String, u8)> {
        members.iter().map(|&(n, f)| (n.to_string(), f)).collect()
    }

    /// `0x49c7e0`'s entries: `*` owner (`0x01`), `@` moderator (`0x02`), `#` for a member
    /// without `0x04` only when the channel's flags have `0x02` (`0x49c8b2`-`0x49c8e0`), and
    /// `", "` before every entry but a line's first (`0x49c8e5`-`0x49c8f6`).
    #[test]
    fn the_entries_take_their_prefixes_and_separators() {
        let members = named(&[
            ("Owner", 0x03),
            ("Mod", 0x02),
            ("Voiced", 0x04),
            ("Muted", 0),
        ]);
        assert_eq!(list_lines(0, &members), ["*Owner, @Mod, Voiced, Muted"]);
        assert_eq!(list_lines(0x02, &members), ["*Owner, @Mod, Voiced, #Muted"]);
        assert!(list_lines(0x02, &[]).is_empty(), "no member, no line");
    }

    /// An entry that would bring the line to 256 bytes posts the line first and opens the next
    /// without its `", "` (`0x49c9b0`-`0x49ca10`): 12-letter names make 14-byte entries, so the
    /// first line holds 18 of them (250 bytes) and the 19th opens the second.
    #[test]
    fn a_line_splits_before_it_reaches_256_bytes() {
        let names: Vec<(String, u8)> = (0..20).map(|i| (format!("Member{i:06}"), 0)).collect();
        let lines = list_lines(0, &names);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].len(), 12 + 17 * 14);
        assert!(lines[0].ends_with("Member000017"));
        assert_eq!(lines[1], "Member000018, Member000019");
    }

    /// The printer waits while a name is unknown (`0x49c81d`-`0x49c85a`); the callback's walk
    /// drops a member whose name the server refused (`0x49d03b`-`0x49d087`) and prints the rest.
    #[test]
    fn a_list_waits_for_its_names_and_drops_a_refused_one() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let chat_log = ChatLog::default();
        let mut names = NameCache::default();
        names.insert_player(1, "Ann".into(), None);
        let mut rosters = ChannelRosters::default();
        let members = [(1, 0), (2, 0), (3, 0)];
        receive(
            &mut rosters,
            Some(1),
            &names,
            &commands,
            &chat_log,
            "Trade",
            0,
            &members,
        );
        assert!(chat_log.pending_events().is_empty(), "two names unknown");
        let asked: Vec<_> = rx.try_iter().collect();
        assert_eq!(asked.len(), 2, "each unknown name asked once: {asked:?}");

        let slot = |n: u32| (n == 1).then(|| "Trade - City".to_string());
        names.insert_player(2, "Bob".into(), None);
        print_answered(&mut rosters, slot, &names, &chat_log);
        assert!(
            chat_log.pending_events().is_empty(),
            "one name still unknown"
        );

        names.insert_player(3, String::new(), None);
        print_answered(&mut rosters, slot, &names, &chat_log);
        let events = chat_log.pending_events();
        assert_eq!(
            events
                .iter()
                .map(|e| (e.text.as_str(), e.channel.as_str()))
                .collect::<Vec<_>>(),
            [("Ann, Bob", "Trade - City")]
        );
        print_answered(&mut rosters, slot, &names, &chat_log);
        assert_eq!(chat_log.pending_events().len(), 1, "the list is freed");
    }
}
