//! The chat send types and the joined-channel slots, beside the stock `ChatEdit_*` machine
//! (`ChatFrame.lua:1782-2242`) that owns the edit box.

use bevy::prelude::*;

use benilla_ui::script::s_str_to_int;

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
    /// `RAID` sent by the group leader (`0x49f3bf`); no token names it.
    RaidLeader,
    RaidWarning,
    Guild,
    Officer,
    Battleground,
    /// `BATTLEGROUND` sent by the group leader (`0x49f48b`); no token names it.
    BattlegroundLeader,
    Afk,
    Dnd,
    Channel,
}

impl SendType {
    /// The `SendChatMessage` chat-type token, one of [`benilla_ui::script::SEND_CHAT_TOKENS`]
    /// (the table `0x49f7a0`); `None` for any other. `AFK` and `DND` are sends: types
    /// `0x14`/`0x15` carrying the away message, which the server turns into the flag (vmangos
    /// `ChatHandler.cpp:611-648`).
    pub(crate) fn from_token(token: &str) -> Option<SendType> {
        Some(match token {
            "SAY" => SendType::Say,
            "YELL" => SendType::Yell,
            "EMOTE" => SendType::Emote,
            "WHISPER" => SendType::Whisper,
            "PARTY" => SendType::Party,
            "RAID" => SendType::Raid,
            "RAID_WARNING" => SendType::RaidWarning,
            "GUILD" => SendType::Guild,
            "OFFICER" => SendType::Officer,
            "BATTLEGROUND" => SendType::Battleground,
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
#[derive(Resource, Default, Clone)]
pub(crate) struct ChannelState {
    /// Slot `i` is channel number `i + 1`; a freed record stays, so the numbers above it do not
    /// move. Never longer than [`MAX_CHANNELS`].
    pub joined: Vec<SlotRecord>,
    /// `ChatChannels.dbc`: it composes the auto-join names and names a slot's row by its id.
    /// Empty without an install.
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
    /// reference writes it from its confirmed id-0 slots at teardown (`0x499b90`-`0x499bb1`); ours
    /// are cleared before the flush, so the list is kept here: seated from the file, grown by a
    /// confirmed custom join, shrunk by an explicit leave and by the free of its slot.
    pub custom: Vec<String>,
    /// The chat cache's `CHANNELS` names still to join, the reference's deferred-join list
    /// (`0xb4fdd8` count, `0xb4fddc` names): the walk's third pass registers and joins each with
    /// id 0 after the zone channels (`0x49a5b4`-`0x49a675`), then frees the list (`0x49a692`).
    pub deferred_joins: Vec<String>,
    /// The zone-defense flag `0x49bd50` stores (`0xb6e5dc`), the world-state readout's `Type` 1
    /// gate. Only a confirm (`0x49bbbf`) and a free (`0x49bc41`) recompute it, so it holds
    /// through a rename, a suspend or a pending join.
    pub defense_channel: bool,
}

/// One slot, the reference's `[0xb4fe04] + n*0xa0` record, in the fields this client uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ChannelSlot {
    /// `+0x04`, the current name; the zone walk renames it in place.
    pub name: String,
    /// `+0x94`, the `ChatChannels.dbc` ChannelID the allocator stored (`0x49ba01`, `0x49baf9`): the
    /// walk's row for a zone channel, the join's matched row, 0 for a custom channel.
    pub id: u32,
    /// `+0x9c`, the slot's state.
    pub state: SlotState,
}

/// One record of the slot array: a channel holds it, or the free `0x49bbd0` zeroed its number and
/// name and kept its `+0x94` (`0x49bc1b`-`0x49bc35`), which the walk's by-id lookup `0x49bef0`
/// still matches, as it tests no number (`0x49bf10`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SlotRecord {
    Held(ChannelSlot),
    Freed { id: u32 },
}

impl SlotRecord {
    /// The channel holding the record; `None` for a freed one.
    pub(crate) fn held(&self) -> Option<&ChannelSlot> {
        match self {
            SlotRecord::Held(slot) => Some(slot),
            SlotRecord::Freed { .. } => None,
        }
    }

    fn held_mut(&mut self) -> Option<&mut ChannelSlot> {
        match self {
            SlotRecord::Held(slot) => Some(slot),
            SlotRecord::Freed { .. } => None,
        }
    }

    /// `+0x94`, held or freed.
    pub(crate) fn id(&self) -> u32 {
        match self {
            SlotRecord::Held(slot) => slot.id,
            SlotRecord::Freed { id } => *id,
        }
    }
}

impl From<ChannelSlot> for SlotRecord {
    fn from(slot: ChannelSlot) -> Self {
        SlotRecord::Held(slot)
    }
}

/// The reference's per-slot state (`+0x9c`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SlotState {
    /// `0`, confirmed by the server's `YOU_JOINED` (`0x49bb8b`): the one state the by-number
    /// lookups (`0x49be50`, `0x49bf30`), the lists and the walk's LEAVE (`0x49a35e`) take.
    Confirmed,
    /// `1`, the join sent and not yet confirmed: the allocator's state (`0x49ba0f`, `0x49bb05`).
    Pending,
    /// `2`, renamed with its re-join pending (`0x49bcd3`, in the rename `0x49bc50`): the confirming
    /// `YOU_JOINED` carries the `YOU_CHANGED` token, "Changed Channel: [%s]".
    Renamed,
    /// `3`, suspended (`0x49bcf0`): the row stopped applying, in 1.12 data a city-only channel
    /// outside a capital. The record and number survive, and `YOU_LEFT` carries the `SUSPENDED`
    /// token (`0x49c0e0`), which keeps the window's registration.
    Suspended,
}

impl ChannelSlot {
    /// A slot the allocator fills: `name`, row `id`, state 1 (`0x49b9e0`-`0x49ba0f`).
    pub(crate) fn pending(name: &str, id: u32) -> Self {
        ChannelSlot {
            name: name.to_string(),
            id,
            state: SlotState::Pending,
        }
    }

    /// A slot the server has confirmed.
    #[cfg(test)]
    pub(crate) fn confirmed(name: &str, id: u32) -> Self {
        ChannelSlot {
            state: SlotState::Confirmed,
            ..Self::pending(name, id)
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
    /// An explicit leave clears the bit (`0x49f10a`/`0x49f11a` in leave-by-name `0x49ee70`) of the
    /// slot carrying the wire name, in any state (`0x49f0f4`), by that slot's id.
    pub(crate) fn note_zone_channel_left(&mut self, name: &str) {
        let Some(id) = self.slot(name).map(|s| s.id) else {
            return;
        };
        if let Some(mask) = &mut self.zone_mask {
            *mask &= !zone_bit(id);
        }
    }

    /// An explicit leave drops a custom channel, case-insensitively as the server's names are.
    pub(crate) fn note_custom_channel_left(&mut self, name: &str) {
        self.custom.retain(|c| !c.eq_ignore_ascii_case(name));
    }

    /// Whether the mask carries `id`'s bit, the walk's live predicate `0x49a494`.
    pub(crate) fn zone_row_wanted(&self, id: u32) -> bool {
        self.zone_mask.is_some_and(|mask| mask & zone_bit(id) != 0)
    }

    /// `ParseChannelKey` (`0x49e880`), the channel argument of leave-by-name and the admin verbs: a
    /// nonzero `SStrToInt` names a confirmed slot (`0x49be50`), and a hole, an out-of-range number
    /// or a slot in another state make the whole call a no-op (`None`). Anything else is already
    /// the wire name.
    pub(crate) fn channel_key(&self, arg: &str) -> Option<String> {
        match s_str_to_int(arg) {
            0 => Some(arg.to_string()),
            n => self.confirmed_slot_name(n).map(str::to_string),
        }
    }

    /// The by-number getter `0x49be50`: slot `n`'s name when it is in range, not a hole, and in
    /// state 0; below 1, past the end, a hole or another state is `None`.
    fn confirmed_slot_name(&self, n: i32) -> Option<&str> {
        usize::try_from(n)
            .ok()
            .and_then(|n| n.checked_sub(1))
            .and_then(|i| self.joined.get(i))
            .and_then(SlotRecord::held)
            .filter(|slot| slot.state == SlotState::Confirmed)
            .map(|slot| slot.name.as_str())
    }

    /// Rename a slot in place at send time, the reference's `0x49bc50` from the walk (`0x49a3dc`):
    /// the server's `YOU_LEFT` for the old name then finds no slot (`0x49be90`), its args default
    /// (`0x49b12f`), and the stock `ChatFrame_OnEvent` returns before its `YOU_LEFT` arm deletes
    /// the window's channel registration (`ChatFrame.lua:1382-1384`), which nothing re-adds.
    pub(crate) fn rename_slot(&mut self, old: &str, new: &str) -> Option<u32> {
        let n = self.number_of(old)?;
        let slot = self.joined[n as usize - 1].held_mut()?;
        slot.name = new.to_string();
        // `0x49bcd3`: `(old == 3) ? 1 : 2`. A suspended slot comes back as a pending join, so its
        // notice reads `YOU_JOINED`; any other is a rename awaiting its re-join.
        slot.state = if slot.state == SlotState::Suspended {
            SlotState::Pending
        } else {
            SlotState::Renamed
        };
        Some(n)
    }

    /// Suspend the slot holding `name` (`0x49bcf0`): the LEAVE has gone, and keeping the record
    /// makes the returning notice `SUSPENDED`, so the window keeps its registration.
    pub(crate) fn suspend_slot(&mut self, name: &str) -> Option<u32> {
        let n = self.number_of(name)?;
        self.joined[n as usize - 1].held_mut()?.state = SlotState::Suspended;
        Some(n)
    }

    /// The `YOU_JOINED` arm's confirm (`0x49c0c2` → `0x49bb20`): the first slot of the name not in
    /// state 0 (`0x49bb43`-`0x49bb6c`) takes state 0 (`0x49bb8b`) and its id's mask bit
    /// (`0x49bbaf`); a name with no such slot changes nothing, so no notice creates a slot. A
    /// confirmed custom slot enters [`Self::custom`], as the teardown writes the confirmed id-0
    /// slots (`0x499b90`-`0x499baa`). Returns the slot's number.
    pub(crate) fn confirm_slot(&mut self, name: &str) -> Option<u32> {
        let i = self.joined.iter().position(|c| {
            c.held().is_some_and(|c| {
                c.state != SlotState::Confirmed && c.name.eq_ignore_ascii_case(name)
            })
        })?;
        let slot = self.joined[i].held_mut()?;
        slot.state = SlotState::Confirmed;
        let (id, name) = (slot.id, slot.name.clone());
        let bit = zone_bit(id);
        match &mut self.zone_mask {
            Some(mask) => *mask |= bit,
            // No join precedes the cache loader, so this is a broken ordering.
            None if bit != 0 => warn!(
                "chat: {name:?} joined before the zone mask was seated — bit {bit:#x} dropped"
            ),
            None => {}
        }
        if id == 0 && !self.custom.iter().any(|c| c.eq_ignore_ascii_case(&name)) {
            self.custom.push(name);
        }
        self.recompute_defense_channel();
        Some(i as u32 + 1)
    }

    /// `0x49bd50`: whether a slot with a number (`0x49bd72`) in state 0 (`0x49bd7c`) has a
    /// `+0x94` id naming a row (`0x49bd83`-`0x49bd96`) with both `ZONE_DEP` and `DEFENSE`
    /// (`0x49bd9b`, `0x49bda2`), which is row 22 alone; a custom channel's id 0 names no row.
    pub(crate) fn recompute_defense_channel(&mut self) {
        use benilla_formats::chat_channel_flags::{DEFENSE, ZONE_DEP};
        let joined = self
            .confirmed_slots()
            .filter_map(|(_, slot)| self.channels.row(slot.id))
            .any(|row| row.flags & (ZONE_DEP | DEFENSE) == ZONE_DEP | DEFENSE);
        self.defense_channel = joined;
    }

    /// The slot holding `name` (case-insensitive) in any state, the lookup `0x49be90`.
    fn slot(&self, name: &str) -> Option<&ChannelSlot> {
        self.number_of(name)
            .and_then(|n| self.joined[n as usize - 1].held())
    }

    /// The state of the slot holding `name`; `None` is the reference's no-record leg.
    pub(crate) fn slot_state(&self, name: &str) -> Option<SlotState> {
        self.slot(name).map(|s| s.state)
    }

    /// The slots for the VM's mirror, holes kept so `/N` addresses the right channel, each with
    /// its id and whether it is in state 0, which the by-number lookups `0x49be50` and `0x49bf30`
    /// and the lists require.
    pub(crate) fn names(&self) -> Vec<Option<benilla_ui::script::JoinedChannel>> {
        self.joined
            .iter()
            .map(|s| {
                s.held().map(|s| benilla_ui::script::JoinedChannel {
                    name: s.name.clone(),
                    id: s.id,
                    confirmed: s.state == SlotState::Confirmed,
                })
            })
            .collect()
    }

    /// Push the slots into the VM's mirror, which `SendChatMessage`'s channel target and the
    /// channel verbs read where the reference reads the slot array itself. Every step that writes
    /// a slot's name or state ends with this, so the mirror never lags the slot it copies.
    pub(crate) fn mirror_into(&self, script: &mut benilla_ui::script::UiScript) {
        script.set_joined_channels(self.names());
    }

    /// The slots in state 0 with their numbers, the set the lists (`0x4a01f2`, `0x4a0302`) and
    /// the defense broadcasts (`0x49dd5a`, `0x49deaa`) walk.
    pub(crate) fn confirmed_slots(&self) -> impl Iterator<Item = (u32, &ChannelSlot)> {
        self.joined
            .iter()
            .enumerate()
            .filter_map(|(i, s)| Some((i as u32 + 1, s.held()?)))
            .filter(|(_, s)| s.state == SlotState::Confirmed)
    }

    /// The 1-based number of `name` (case-insensitive), if joined.
    pub(crate) fn number_of(&self, name: &str) -> Option<u32> {
        self.joined
            .iter()
            .position(|c| c.held().is_some_and(|c| c.name.eq_ignore_ascii_case(name)))
            .map(|i| i as u32 + 1)
    }

    /// Whether any record, held or freed, carries row `id`: the walk's `0x49bef0`, which compares
    /// `+0x94` alone (`0x49bf10`).
    pub(crate) fn carries_row(&self, id: u32) -> bool {
        self.joined.iter().any(|r| r.id() == id)
    }

    /// The allocator `0x49b980`, run when a join is sent: a name already held keeps its slot as it
    /// is (`0x49b991`); otherwise the first freed record (`0x49b9b0`), whose id it overwrites
    /// (`0x49ba01`), else a new one under [`MAX_CHANNELS`], takes the name and row `id` in state 1
    /// (`0x49ba0f`, `0x49bb05`). With all ten taken it makes none (`0x49b9c0`), and the caller
    /// shows `ERR_TOO_MANY_CHAT_CHANNELS` (`0x199`, `0x49b9c5`) and sends the join anyway.
    pub(crate) fn claim_slot(&mut self, name: &str, id: u32) -> Option<u32> {
        if let Some(n) = self.number_of(name) {
            return Some(n);
        }
        let slot = SlotRecord::Held(ChannelSlot::pending(name, id));
        if let Some(i) = self.joined.iter().position(|r| r.held().is_none()) {
            self.joined[i] = slot;
            return Some(i as u32 + 1);
        }
        if self.joined.len() >= MAX_CHANNELS {
            return None;
        }
        self.joined.push(slot);
        Some(self.joined.len() as u32)
    }

    /// [`Self::claim_slot`] with its refusal shown: `DisplayError(0x199)` (`0x49b9c5`), a chat
    /// row (`CHAT_MSG_SYSTEM`).
    pub(crate) fn claim_or_refuse(
        &mut self,
        name: &str,
        id: u32,
        errors: &crate::ui_action::UiErrorKeys,
    ) -> Option<u32> {
        let n = self.claim_slot(name, id);
        if n.is_none() {
            debug!("chat: no free slot for {name:?} — all {MAX_CHANNELS} are taken");
            errors.push(crate::ui_action::UiError::key("ERR_TOO_MANY_CHAT_CHANNELS"));
        }
        n
    }

    /// Free the slot holding `name` in place (`0x49bbd0`), so no other channel renumbers; the
    /// record keeps its id. A freed custom slot leaves [`Self::custom`]: the teardown writes only
    /// live slots (`0x499b90`).
    pub(crate) fn free_slot(&mut self, name: &str) -> Option<u32> {
        let n = self.number_of(name)?;
        let record = &mut self.joined[n as usize - 1];
        let id = record.id();
        if let SlotRecord::Held(slot) = std::mem::replace(record, SlotRecord::Freed { id }) {
            if slot.id == 0 {
                self.note_custom_channel_left(&slot.name);
            }
        }
        self.recompute_defense_channel();
        Some(n)
    }

    /// The stamp of the record holding `name`, as the reference reads it off one record
    /// (`0x49b113`-`0x49b12a`: `+0x00` number, `+0x04` name, `+0x94` ChannelID, `+0x98` split
    /// index); `None` for an empty name or one no slot holds (`0x49aa86`, `0x49b12f`).
    pub(crate) fn stamp_of(&self, name: &str) -> Option<ChannelStamp> {
        let number = self.number_of(name).filter(|_| !name.is_empty())?;
        Some(ChannelStamp {
            number,
            id: self.joined[number as usize - 1].id(),
        })
    }

    /// Stamp an event's channel args from our slot ([`ChannelStamp::apply`]); a miss stamps
    /// nothing, as the four args are one record.
    pub(crate) fn stamp_channel(&self, event: &mut super::event::ChatEvent) {
        if let Some(stamp) = self.stamp_of(&event.channel) {
            stamp.apply(event);
        }
    }
}

/// What a channel line's record stamps on it ([`ChannelState::stamp_of`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ChannelStamp {
    number: u32,
    id: u32,
}

impl ChannelStamp {
    /// Stamp `event`, whose `channel` is the record's name: arg4 becomes `"N. Name"` (`0x8445c8`,
    /// the hit leg `0x49aa48`), arg8 the number, arg9 the name with its zone tail and arg7 the
    /// slot's id.
    pub(crate) fn apply(self, event: &mut super::event::ChatEvent) {
        event.channel_base = event.channel.clone();
        event.zone_channel_id = self.id;
        event.channel_number = self.number;
        event.channel = format!("{}. {}", self.number, event.channel_base);
    }
}
