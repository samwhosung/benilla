//! The chat send types and the joined-channel slots, beside the stock `ChatEdit_*` machine
//! (`ChatFrame.lua:1782-2242`) that owns the edit box.

use bevy::prelude::*;

use crate::net::ChatKind;

/// The sendable chat types, as the wire kind an addon's `SendChatMessage` token maps to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SendType {
    Say,
    Yell,
    Emote,
    Whisper,
    Party,
    Raid,
    RaidLeader,
    RaidWarning,
    Guild,
    Officer,
    Battleground,
    BattlegroundLeader,
    Afk,
    Dnd,
    Channel,
}

impl SendType {
    /// The `SendChatMessage` chat-type token; `None` for a made-up one, reported rather than sent
    /// as SAY. `AFK` and `DND` are sends: types `0x14`/`0x15` carrying the away message, which the
    /// server turns into the flag (vmangos `ChatHandler.cpp:611-648`).
    pub(crate) fn from_token(token: &str) -> Option<SendType> {
        Some(match token {
            "SAY" => SendType::Say,
            "YELL" => SendType::Yell,
            "EMOTE" => SendType::Emote,
            "WHISPER" => SendType::Whisper,
            "PARTY" => SendType::Party,
            "RAID" => SendType::Raid,
            "RAID_LEADER" => SendType::RaidLeader,
            "RAID_WARNING" => SendType::RaidWarning,
            "GUILD" => SendType::Guild,
            "OFFICER" => SendType::Officer,
            "BATTLEGROUND" => SendType::Battleground,
            "BATTLEGROUND_LEADER" => SendType::BattlegroundLeader,
            "AFK" => SendType::Afk,
            "DND" => SendType::Dnd,
            "CHANNEL" => SendType::Channel,
            _ => return None,
        })
    }

    /// The wire kind this type sends as.
    pub(crate) fn wire(self) -> ChatKind {
        match self {
            SendType::Say => ChatKind::Say,
            SendType::Yell => ChatKind::Yell,
            SendType::Emote => ChatKind::Emote,
            SendType::Whisper => ChatKind::Whisper,
            SendType::Party => ChatKind::Party,
            SendType::Raid => ChatKind::Raid,
            SendType::RaidLeader => ChatKind::RaidLeader,
            SendType::RaidWarning => ChatKind::RaidWarning,
            SendType::Guild => ChatKind::Guild,
            SendType::Officer => ChatKind::Officer,
            SendType::Battleground => ChatKind::Battleground,
            SendType::BattlegroundLeader => ChatKind::BattlegroundLeader,
            SendType::Afk => ChatKind::Afk,
            SendType::Dnd => ChatKind::Dnd,
            SendType::Channel => ChatKind::Channel,
        }
    }
}

/// The client refuses an eleventh channel (`0x49b9c0: cmp ecx,0xa`); the boot-seeded
/// `CHANNEL1`-`CHANNEL10` colour rows are the same ten (`0x4982c0`).
pub(crate) const MAX_CHANNELS: usize = 10;

/// The joined channels as the reference's slot array at `[0xb4fe04]` (stride `0xa0`, number at
/// `+0x00`): `/N` is slot N. The allocator `0x49b980` reuses the first free entry before growing,
/// and a leave zeroes the number in place (`0x49bbd0`), so no other channel is renumbered and a
/// hole answers "not joined" (`0x49bf30`).
#[derive(Resource, Default)]
pub(crate) struct ChannelState {
    /// Slot `i` is channel number `i + 1`; `None` is a freed slot, kept so the numbers above it
    /// do not move. Never longer than [`MAX_CHANNELS`].
    pub joined: Vec<Option<ChannelSlot>>,
    /// `ChatChannels.dbc`: it composes the auto-join names and resolves a chat event's arg7 from a
    /// name, as the server does. Empty without an install.
    pub channels: benilla_formats::ChatChannelsCatalog,
    /// The `ZONECHANNELS` mask, the reference's `ds:0xb6e5e0`, bit `1 << (ChannelID - 1)`: seeded
    /// from the chat cache (`0x498d83`) or the `INITIAL` rows (`0x4997fc`), set by a confirmed join
    /// (`0x49bbaf`) and cleared only by an explicit leave (`0x49f10a`), never the walk's LEAVE.
    /// Never derive it from the roster: each saved window's channel bits are ANDed with it, so an
    /// empty roster would save every window without its channels.
    ///
    /// `None` until the login reads the file: the reference's ready flag `ds:0xb6e5c8`, set by the
    /// cache loader (`0x499a18`) and tested first by the walk (`0x49a219`); the saver writes
    /// nothing from `None`.
    pub zone_mask: Option<u32>,
    /// The custom channels to re-join at the next login, the chat cache's `CHANNELS` list. The
    /// reference writes it from its live slots at teardown (`0x499b90`-`0x499bb1`); ours are
    /// cleared before the flush, so the list is kept here: seated from the file, grown by a
    /// confirmed custom join, shrunk by an explicit leave. A channel whose re-join is refused, or
    /// that kicks the player, stays listed until a `/leave`, where the reference's file drops it.
    pub custom: Vec<String>,
}

/// One slot, the reference's `[0xb4fe04] + n*0xa0` record, in the fields this client uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ChannelSlot {
    /// `+0x04`, the current name; the zone walk renames it in place.
    pub name: String,
    /// `+0x9c`, the slot's state.
    pub state: SlotState,
}

/// The reference's per-slot state (`+0x9c`), in the values whose notice token differs: the stock
/// `ChatFrame_OnEvent`'s `YOU_LEFT` branch deletes the window's channel registration.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SlotState {
    /// `0` (confirmed) and `1` (join sent) in one: the reference's walk sends a LEAVE only from 0,
    /// so an unconfirmed slot here sends one the server answers "Not on channel".
    #[default]
    Joined,
    /// `2`, renamed with its re-join pending (`0x49bcd3`, in the rename `0x49bc50`): the confirming
    /// `YOU_JOINED` carries the `YOU_CHANGED` token, "Changed Channel: [%s]".
    Renamed,
    /// `3`, suspended (`0x49bcf0`): the row stopped applying, in 1.12 data a city-only channel
    /// outside a capital. The record and number survive, and `YOU_LEFT` carries the `SUSPENDED`
    /// token (`0x49c0e0`), which keeps the window's registration.
    Suspended,
}

impl ChannelSlot {
    /// A new slot in state `Joined`: claimed by the walk at send time or by a confirmed join.
    pub(crate) fn joined(name: &str) -> Self {
        ChannelSlot {
            name: name.to_string(),
            state: SlotState::Joined,
        }
    }
}

/// `id`'s `ZONECHANNELS` bit, `1 << (ChannelID - 1)`; nothing outside `1..=32` has one.
pub(crate) fn zone_bit(id: u32) -> u32 {
    if id == 0 || id > 32 {
        0
    } else {
        1 << (id - 1)
    }
}

impl ChannelState {
    /// A confirmed join sets the channel's mask bit (`0x49bbaf`, in the `YOU_JOINED` arm).
    pub(crate) fn note_zone_channel_joined(&mut self, name: &str) {
        let bit = zone_bit(self.channels.zone_channel_id(name));
        match &mut self.zone_mask {
            Some(mask) => *mask |= bit,
            // No join precedes the cache loader, so this is a broken ordering.
            None if bit != 0 => warn!(
                "chat: {name:?} joined before the zone mask was seated — bit {bit:#x} dropped"
            ),
            None => {}
        }
    }

    /// An explicit leave clears the bit (`0x49f10a`/`0x49f11a` in leave-by-name `0x49ee70`), found
    /// as the reference finds it (`0x49f0f4`): by the slot carrying the wire name.
    pub(crate) fn note_zone_channel_left(&mut self, name: &str) {
        if self.number_of(name).is_none() {
            return;
        }
        if let Some(mask) = &mut self.zone_mask {
            *mask &= !zone_bit(self.channels.zone_channel_id(name));
        }
    }

    /// A confirmed join of a channel with no `ChatChannels.dbc` id enters [`Self::custom`].
    pub(crate) fn note_custom_channel_joined(&mut self, name: &str) {
        if self.channels.zone_channel_id(name) != 0
            || self.custom.iter().any(|c| c.eq_ignore_ascii_case(name))
        {
            return;
        }
        self.custom.push(name.to_string());
    }

    /// An explicit leave drops a custom channel, case-insensitively as the server's names are.
    pub(crate) fn note_custom_channel_left(&mut self, name: &str) {
        self.custom.retain(|c| !c.eq_ignore_ascii_case(name));
    }

    /// Whether the mask carries `id`'s bit, the walk's live predicate `0x49a494`.
    pub(crate) fn zone_row_wanted(&self, id: u32) -> bool {
        self.zone_mask.is_some_and(|mask| mask & zone_bit(id) != 0)
    }

    /// The numeric leg of leave-by-name (`0x49ee70`): a nonzero `SStrToInt` names a confirmed slot
    /// (`0x49be50`), and a hole, an out-of-range number or a suspended slot make the whole call a
    /// no-op (`None`). Anything else is already the wire name.
    pub(crate) fn leave_target(&self, arg: &str) -> Option<String> {
        let digits: String = arg
            .strip_prefix('-')
            .unwrap_or(arg)
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if digits.is_empty() || digits.chars().all(|c| c == '0') {
            return Some(arg.to_string());
        }
        if arg.starts_with('-') {
            return None; // a negative never names a slot
        }
        digits
            .parse::<usize>()
            .ok()
            .filter(|n| *n > 0)
            .and_then(|n| self.joined.get(n - 1))
            .and_then(|slot| slot.as_ref())
            .filter(|slot| slot.state == SlotState::Joined)
            .map(|slot| slot.name.clone())
    }

    /// Rename a slot in place at send time, the reference's `0x49bc50` from the walk (`0x49a3dc`):
    /// the server's `YOU_LEFT` for the old name then finds no slot (`0x49be90`), its args default
    /// (`0x49b12f`), and the stock `ChatFrame_OnEvent` returns before its `YOU_LEFT` arm deletes
    /// the window's channel registration (`ChatFrame.lua:1382-1384`), which nothing re-adds.
    pub(crate) fn rename_slot(&mut self, old: &str, new: &str) -> Option<u32> {
        let n = self.number_of(old)?;
        let slot = self.joined[n as usize - 1].as_mut()?;
        slot.name = new.to_string();
        // `0x49bcd3`: `(old == 3) ? 1 : 2`. A suspended slot comes back as a pending join, our
        // `Joined`, so its notice reads `YOU_JOINED`; any other is a rename awaiting its re-join.
        slot.state = if slot.state == SlotState::Suspended {
            SlotState::Joined
        } else {
            SlotState::Renamed
        };
        Some(n)
    }

    /// Suspend the slot holding `name` (`0x49bcf0`): the LEAVE has gone, and keeping the record
    /// makes the returning notice `SUSPENDED`, so the window keeps its registration.
    pub(crate) fn suspend_slot(&mut self, name: &str) -> Option<u32> {
        let n = self.number_of(name)?;
        self.joined[n as usize - 1].as_mut()?.state = SlotState::Suspended;
        Some(n)
    }

    /// The confirming `YOU_JOINED` writes state 0 unconditionally (`0x49bb20`); separate from
    /// [`Self::claim_slot`], as a renamed slot is already numbered.
    pub(crate) fn confirm_slot(&mut self, name: &str) {
        if let Some(n) = self.number_of(name) {
            if let Some(slot) = self.joined[n as usize - 1].as_mut() {
                slot.state = SlotState::Joined;
            }
        }
    }

    /// The state of the slot holding `name`; `None` is the reference's no-record leg.
    pub(crate) fn slot_state(&self, name: &str) -> Option<SlotState> {
        self.number_of(name)
            .and_then(|n| self.joined[n as usize - 1].as_ref())
            .map(|s| s.state)
    }

    /// The names for the VM's mirror, holes kept so `/N` addresses the right channel.
    pub(crate) fn names(&self) -> Vec<Option<String>> {
        self.joined
            .iter()
            .map(|s| s.as_ref().map(|s| s.name.clone()))
            .collect()
    }

    /// Every joined channel's name, holes skipped.
    pub(crate) fn iter_names(&self) -> impl Iterator<Item = &str> {
        self.joined.iter().flatten().map(|s| s.name.as_str())
    }

    /// The 1-based number of `name` (case-insensitive), if joined.
    pub(crate) fn number_of(&self, name: &str) -> Option<u32> {
        self.joined
            .iter()
            .position(|c| {
                c.as_ref()
                    .is_some_and(|c| c.name.eq_ignore_ascii_case(name))
            })
            .map(|i| i as u32 + 1)
    }

    /// Give `name` the first free slot, else a new one under [`MAX_CHANNELS`] (`0x49b980`); a name
    /// already held keeps its number. With all ten taken, the reference prints
    /// `ERR_TOO_MANY_CHAT_CHANNELS` (`0x199`, `0x49b9c5` via `0x496720`) and still sends the join;
    /// here the callers only log it.
    pub(crate) fn claim_slot(&mut self, name: &str) -> Option<u32> {
        if let Some(n) = self.number_of(name) {
            // A confirmed join on a held slot clears its suspension (`0x49bb20` writes state 0
            // unconditionally): the other half of the state-3 bypass.
            if let Some(slot) = self.joined[n as usize - 1].as_mut() {
                slot.state = SlotState::Joined;
            }
            return Some(n);
        }
        if let Some(i) = self.joined.iter().position(Option::is_none) {
            self.joined[i] = Some(ChannelSlot::joined(name));
            return Some(i as u32 + 1);
        }
        if self.joined.len() >= MAX_CHANNELS {
            return None;
        }
        self.joined.push(Some(ChannelSlot::joined(name)));
        Some(self.joined.len() as u32)
    }

    /// Free the slot holding `name` in place (`0x49bbd0`), so no other channel renumbers.
    pub(crate) fn free_slot(&mut self, name: &str) -> Option<u32> {
        let n = self.number_of(name)?;
        self.joined[n as usize - 1] = None;
        Some(n)
    }

    /// Stamp an event's channel args from our slot, as the reference reads them off one record
    /// (`+0x00` number, `+0x04` name, `+0x94` ChannelID, `+0x98` split index): held, arg4 becomes
    /// `"N. Name"` (`0x8445c8`, the hit leg `0x49aa48`), arg8 the number, arg9 the name with its
    /// zone tail and arg7 the ChannelID; not held, nothing is stamped (`0x49aa86`, `0x49b12f`).
    /// arg7 comes from the name, as vmangos resolves it (`DBCStores.cpp:531`).
    pub(crate) fn stamp_channel(&self, event: &mut super::event::ChatEvent) {
        // A miss stamps nothing: the four args are one record.
        let Some(n) = self
            .number_of(&event.channel)
            .filter(|_| !event.channel.is_empty())
        else {
            return;
        };
        event.channel_base = event.channel.clone();
        event.zone_channel_id = self.channels.zone_channel_id(&event.channel_base);
        event.channel_number = n;
        event.channel = format!("{n}. {}", event.channel_base);
    }
}
