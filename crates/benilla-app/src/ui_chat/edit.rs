//! The chat SEND types and the joined-channel roster — what the app keeps beside the reference's
//! own `ChatEdit_*` machine (ChatFrame.lua l.1782-2242), which owns the edit box since the chat
//! window became the reference's: the sticky type, the live parse, the header,
//! the tell ring, the Tab cycle and the R/`/` bindings are all its Lua now.
//!
//! [`SendType`] names the wire kind an addon's `SendChatMessage` token maps to
//! ([`super::input::drain_addon_chat_sends`]); [`ChannelState`] is the client-side mirror of the
//! joined channels (the `/N` numbering the reference keeps C-side).

use bevy::prelude::*;

use crate::net::ChatKind;

/// The sendable chat types — `ChatTypeInfo`'s sendable keys, as the wire kind an addon's
/// `SendChatMessage` token maps to. `Whisper`/`Channel` carry their target in the call.
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
    /// The chat-type TOKEN an addon passes to `SendChatMessage` — the reference's
    /// own `ChatTypeInfo` keys, uppercase.
    ///
    /// `None` for a token we do not send — anything an addon simply made up. The caller reports
    /// it rather than guessing SAY, because a raid warning silently going to /say is worse than
    /// one that does not go.
    ///
    /// **`"AFK"` and `"DND"` ARE sends, and this said the opposite** — "which set a flag rather
    /// than sending a line". They are `CMSG_MESSAGECHAT` types `0x14`/`0x15` like every other row
    /// here, carrying the away message as their body; it is the SERVER that toggles the
    /// `PLAYER_FLAGS` bit off the packet and streams it back (vmangos `ChatHandler.cpp`'s
    /// `CHAT_MSG_AFK` arm → `Player::ToggleAFK`). The wire half was already built and reachable —
    /// `ChatKind::Afk`/`Dnd`, `writer::chat::send_afk`/`send_dnd`, `CHAT_TYPE_AFK` — and this
    /// function was the only thing standing between the stock file and it.
    ///
    /// **What that cost, and why it is 1751's own lesson:** benilla's slash grammar has always
    /// had a working `/afk` (`S::ChatAfk` → `ParsedChat::AfkDnd`). Migrating the chat window
    /// (1948) put the stock `ChatFrame.lua` on the chain, and its parser claims a slash line
    /// before benilla's grammar ever sees it — so `/afk` became
    /// `SlashCmdList["CHAT_AFK"](msg)` → `SendChatMessage(msg, "AFK")` → here → `None`, and the
    /// player got `Unknown chat type "AFK".` Migrating a window means building whatever engine
    /// verb the stock file turns out to call; the verb existed, and a wrong sentence in this doc
    /// comment is what kept it unreachable.
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

/// How many channels the client can hold at once — its allocator refuses the eleventh
/// (`0x49b9c0: cmp ecx,0xa`), and the ten boot-seeded `CHANNEL1`…`CHANNEL10` color rows are the
/// same ten (seeded by `0x4982c0`).
pub(crate) const MAX_CHANNELS: usize = 10;

/// The channels this session has joined — the CLIENT-side number law (`GetChannelName(n)`): `/1`
/// is slot 1, `/2` slot 2; the numbered display form ("1. General - Elwynn Forest") and the
/// `[N. Name]` prefixes all derive from it. Fed by YOU_JOINED / YOU_LEFT notices
/// ([`super::feed`]); the zone AUTO-join walk that fills it at login is [`super::channels`].
///
/// **It is a SLOT ARRAY, and leaving punches a hole rather than closing one** (1286). The client's
/// records live in a fixed array at `[0xb4fe04]`, stride `0xa0`, with the entry's own **number**
/// at `+0x00`; the allocator `0x49b980` scans for an entry whose number is `0` and *reuses* it
/// (`0x49b9b0`: `cmp dword [edx],0` / `jz`), only growing when none is free and the count is under
/// **ten** (`0x49b9c0: cmp ecx,0xa`), and the leave path `0x49bbd0` clears that number in place
/// (`0x49bc1b: mov dword [eax+edx],0`) without shrinking the count. Lookup by index then demands
/// the entry's number equal the index asked for (`0x49bf30: cmp esi,ecx / jnz`), so a hole answers
/// "not joined" while every channel above it keeps its number.
///
/// A `Vec<String>` cannot express that: `retain` closed the hole and renumbered everything above
/// it, so walking out of a zone renamed *other* channels — the director saw General and
/// LocalDefense trade numbers on one zone change, and a `/2` typed after that went somewhere else.
///
/// **And each slot carries a state, because the reference's does** (`+0x9c`). We
/// model the one value of it that changes what the player sees: **3, locally suspended**. States
/// 0 (server-confirmed), 1 (join not yet acknowledged) and 2 (renamed, re-join pending) collapse
/// here, and that is sound rather than lazy — the reference reads 0 to decide whether a LEAVE goes
/// out, and our walk decides that from the slot's own state on the request side (1284). State 3
/// does not collapse: it is the difference between keeping a channel and losing it.
#[derive(Resource, Default)]
pub(crate) struct ChannelState {
    /// Slot `i` is channel number `i + 1`; `None` is a freed slot, kept so the numbers above it
    /// do not move. Never longer than [`MAX_CHANNELS`].
    pub joined: Vec<Option<ChannelSlot>>,
    /// `ChatChannels.dbc`, loaded once at Startup ([`super::channels::load_chat_channels`]).
    ///
    /// It lives here because both of its consumers are this type's own business: composing the
    /// auto-join names, and answering a chat event's **arg7** — the built-in ChannelID behind a
    /// name, which is a pure function of the name (the server resolves it the same way) and so
    /// needs no extra bookkeeping at join time. Empty without an install, which degrades to
    /// "no zone channels, arg7 always 0" rather than to an error.
    pub channels: benilla_formats::ChatChannelsCatalog,
    /// **The `ZONECHANNELS` mask** — the reference's `DWORD ds:0xb6e5e0`, bit `1 << (ChannelID-1)`
    /// (the global's complete census is eight sites).
    ///
    /// **It is durable state, not a view of [`Self::joined`].** The reference seeds it once — from
    /// the chat cache's header line (`0x498d83`, an overwrite) or, with no usable file, from every
    /// `ChatChannels.dbc` row carrying `INITIAL` (`0x4997fc`) — then ORs a bit on each
    /// server-confirmed join (`0x49bbaf`, the `YOU_JOINED` arm) and clears one only on an explicit
    /// leave-by-name (`0x49f10a`/`0x49f11a` inside `0x49ee70`). The zone walk's own LEAVE, sent
    /// every time you cross a border or walk out of a capital, does **not** touch it — which is
    /// why `Trade`'s bit survives a logout in Elwynn Forest.
    ///
    /// Deriving it from the live roster at write time instead is what decision 2120 corrects, and
    /// it was not cosmetic: the per-window line is written as `the window's own bits AND this
    /// mask`, so one save taken while the roster was momentarily empty — the session-end flush
    /// racing `end_session_channels` on the same unordered `OnExit(InWorld)` edge — wrote
    /// `ZONECHANNELS 0` into every block, and the next login rebuilt window 1 with **no channels
    /// at all**. The stock `ChatFrame_OnEvent` drops every `CHANNEL*` line whose channel the
    /// window does not carry (ref `ChatFrame.lua` l.1374-1391, `if found == 0 … return`), so that
    /// character silently lost its `Joined Channel:` notices *and* all General/Trade speech, for
    /// good. Ten of the twenty files in this repo's own config folder had reached that state,
    /// including the director's own character.
    ///
    /// Not cleared by the session end: it belongs to the character's file, and the login that
    /// reads that file is what seats it.
    ///
    /// **`None` until that login has read the file** — the reference's "chat system ready" flag
    /// `ds:0xb6e5c8`, set at the tail of the cache loader (`0x499a18`) and the first thing
    /// `ZoneChannelRefresh` tests (`0x49a219`, a full bail). The walk is the mask's consumer
    /// (**the mask is the join predicate**, `0x49a494`), so a walk before the seat
    /// would read an empty word and join nothing — and nothing re-triggers it when the word
    /// lands. An `Option` says "not seated" in the type rather than in a second flag that could
    /// drift from it; the saver refuses to compose a file from `None` for the same reason
    /// (writing `ZONECHANNELS 0` is the damage 2120 repaired).
    pub zone_mask: Option<u32>,
    /// **The custom channels the character is in** — the chat cache header's `CHANNELS` list, the
    /// names the next login re-joins. Durable state, for exactly [`Self::zone_mask`]'s reason: the
    /// reference writes that list from its live slot array (`0x499b90`-`0x499bb1`: every slot
    /// with a number, state 0 and no DBC id) at the chat teardown, when its slots are still
    /// there — ours are cleared by the session end *before* the flush (the
    /// `Disconnected{LoggedOut}` twin runs in `Update`, a frame ahead of `OnExit(InWorld)`), so a
    /// list read off [`Self::joined`] at write time was empty on every logout and no custom
    /// channel was ever re-joined (2184 §6).
    ///
    /// So it moves where the slot array's custom half moves, minus the session end: seated from
    /// the file by `restore_chat_looks` (an overwrite, like the mask), grown on a
    /// server-confirmed join of a channel with no DBC id ([`Self::note_custom_channel_joined`]),
    /// shrunk on an explicit leave ([`Self::note_custom_channel_left`]), and nothing else.
    ///
    /// **Named divergence:** a channel whose re-join the server refuses (a ban, a changed
    /// password) or that the player is kicked from never takes a confirmed slot in the reference
    /// and so drops out of its file; here it stays listed until the player `/leave`s it.
    pub custom: Vec<String>,
}

/// One joined-channel record — the reference's `[0xb4fe04] + n*0xa0` slot, in the fields this
/// client uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ChannelSlot {
    /// `+0x04` — the channel's current name. The zone walk RENAMES this in place
    /// ([`ChannelState::rename_slot`]).
    pub name: String,
    /// `+0x9c` — the slot's state, in the three values that change what the player sees.
    pub state: SlotState,
}

/// The reference's per-slot state (`slot+0x9c`), modelled in the values whose **notice token**
/// differs — because the token is what the stock `ChatFrame_OnEvent` branches on, and one of those
/// branches deletes the window's channel registration.
///
/// Its complete writer census is six sites.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SlotState {
    /// `0` (server-confirmed) and `1` (join not yet acknowledged), which we cannot tell apart and
    /// do not need to: the reference reads `0` to decide whether a LEAVE goes out, and our walk
    /// reads this state for the same decision — a slot it registered and never confirmed sends
    /// one LEAVE the server answers "Not on channel", which is the one cost of the collapse.
    #[default]
    Joined,
    /// `2` — **renamed, re-join pending**: the zone walk moved this row's name because the player
    /// crossed a border (`0x49bcd3`, inside the rename `0x49bc50`). The confirming `YOU_JOINED`
    /// then carries the `YOU_CHANGED` token instead, which is `CHAT_YOU_CHANGED_NOTICE` —
    /// *"Changed Channel: [%s]"*, a line the reference has and we never printed.
    Renamed,
    /// `3` — **locally suspended**: the row lost its eligibility, which in the 1.12 data means
    /// exactly one thing, walking out of a capital with `Trade` joined. `0x49bcf0` sets it
    /// and sends nothing; the LEAVE has already gone out earlier in the same iteration.
    ///
    /// The record and its number survive, and the arriving `YOU_LEFT` carries the `SUSPENDED`
    /// token instead (`0x49c0e0`) — same rendered text, different arg1, which is exactly what stops
    /// `ChatFrame_OnEvent` from deleting the registration. Walking back into a city then re-joins
    /// through the state-3 bypass (`0x49a31c`), where the name has not changed and the
    /// comparison would otherwise say there was nothing to do.
    Suspended,
}

impl ChannelSlot {
    /// A server-confirmed slot — the only way one is ever born here, because this client registers
    /// a slot on `YOU_JOINED` rather than at send time (the reference's `0x49b980` does the latter,
    /// at state 1; the difference is invisible to everything we model).
    pub(crate) fn joined(name: &str) -> Self {
        ChannelSlot {
            name: name.to_string(),
            state: SlotState::Joined,
        }
    }
}

/// The bit `id` occupies in a `ZONECHANNELS` word — `1 << (ChannelID - 1)`; nothing outside
/// `1..=32` has one.
pub(crate) fn zone_bit(id: u32) -> u32 {
    if id == 0 || id > 32 {
        0
    } else {
        1 << (id - 1)
    }
}

impl ChannelState {
    /// A server-confirmed join sets the channel's `ZONECHANNELS` bit — the reference's `0x49bbaf`,
    /// which ORs `1 << (slot.ChannelID - 1)` in the `YOU_JOINED` arm. A custom channel has no DBC
    /// id and so no bit, which is why this is a no-op for one.
    pub(crate) fn note_zone_channel_joined(&mut self, name: &str) {
        let bit = zone_bit(self.channels.zone_channel_id(name));
        match &mut self.zone_mask {
            Some(mask) => *mask |= bit,
            // Nothing sends a join before the cache loader has run — the walk waits for the seat
            // and the file's own custom re-joins come after it — so this is a broken ordering,
            // not a state to absorb.
            None if bit != 0 => warn!(
                "chat: {name:?} confirmed joined before the chat cache seated the zone mask — bit                  {bit:#x} dropped"
            ),
            None => {}
        }
    }

    /// An **explicit** leave clears the bit — `0x49f10a`/`0x49f11a` inside leave-by-name
    /// `0x49ee70`, and only there. The zone walk's LEAVE goes out on a different path and leaves
    /// the mask alone: crossing a border is not "I left this channel".
    ///
    /// Keyed the way the reference keys it (`0x49f0f4`): the slot
    /// found by the **wire name**, and its own DBC id — so a name no slot carries clears nothing,
    /// whatever row it would resolve to.
    pub(crate) fn note_zone_channel_left(&mut self, name: &str) {
        if self.number_of(name).is_none() {
            return;
        }
        if let Some(mask) = &mut self.zone_mask {
            *mask &= !zone_bit(self.channels.zone_channel_id(name));
        }
    }

    /// A server-confirmed join of a **custom** channel (no `ChatChannels.dbc` id) enters the
    /// durable re-join list ([`Self::custom`]) — the slot the reference's writer would list at
    /// teardown. A zone channel is a no-op here; it travels as its mask bit instead.
    pub(crate) fn note_custom_channel_joined(&mut self, name: &str) {
        if self.channels.zone_channel_id(name) != 0
            || self.custom.iter().any(|c| c.eq_ignore_ascii_case(name))
        {
            return;
        }
        self.custom.push(name.to_string());
    }

    /// An **explicit** leave drops a custom channel from the re-join list — the one thing
    /// besides the file that shrinks it ([`Self::custom`]). Case-insensitive, as the server's
    /// channel names are.
    pub(crate) fn note_custom_channel_left(&mut self, name: &str) {
        self.custom.retain(|c| !c.eq_ignore_ascii_case(name));
    }

    /// Does the mask carry `id`'s bit — is this `ChatChannels.dbc` row one the walk joins? The
    /// reference's live predicate `0x49a494`. `None` (not seated) answers false.
    pub(crate) fn zone_row_wanted(&self, id: u32) -> bool {
        self.zone_mask.is_some_and(|mask| mask & zone_bit(id) != 0)
    }

    /// **The numeric leg of leave-by-name** — `0x49ee70` step 1: a `SStrToInt` of the argument that
    /// is not zero names joined slot `n`, and only a **server-confirmed** one (`slot+0x9c == 0`,
    /// `0x49be50`); a hole, an out-of-range number or a suspended slot make the whole call a
    /// no-op — no packet, no mask change. `None` is that no-op. Anything else is already the wire
    /// name: the VM's `LeaveChannelByName` composed a shortcut or passed a custom name through.
    ///
    /// Here rather than in the VM because the slot **states** live here; the VM's mirror carries
    /// names alone.
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

    /// **Rename a slot in place — the zone walk crossing a border**.
    ///
    /// The reference's `0x49bc50(oldName, newName)`, called from `ZoneChannelRefresh`'s pass 1 at
    /// `0x49a3dc`: it copies the new name into the slot and moves its state to "re-join pending" —
    /// **at send time**, before the server has answered the `CMSG_LEAVE_CHANNEL` that went out a
    /// moment earlier.
    ///
    /// That ordering is the whole point, and it is not bookkeeping. When the server's `YOU_LEFT`
    /// for the *old* name arrives, no slot carries that name any more — `0x49be90` is a pure name
    /// scan with no state filter — so the marshaller (`0x49b0b0`) takes its NULL-slot leg
    /// (`0x49b12f`) and defaults `arg7`, `arg8`, `arg9` and `arg10` to `0 / 0 / "" / 0` **together**,
    /// and the stock `ChatFrame_OnEvent` finds no match and returns (`found == 0`). Which means it
    /// never reaches its own `YOU_LEFT` arm, and that arm is the one that **deletes the window's
    /// channel registration** (`ChatFrame.lua` l.1382-1384):
    ///
    /// ```lua
    /// this.channelList[index] = nil;
    /// this.zoneChannelList[index] = nil;
    /// ```
    ///
    /// Nothing in stock FrameXML ever re-adds one on a join, and **nothing in the engine does
    /// either**: a closed census of event 395 (`0x18b`) leaves five fire sites in the image, none of
    /// them reachable from a zone change, for guilded and unguilded players alike (`0x49a6a4`).
    /// The rename is not one repopulation mechanism among several — it is the only thing
    /// standing between a border crossing and a dead channel.
    ///
    /// So without it, one crossing costs the window General and LocalDefense **for the rest of the
    /// session** — the replacement join notice dropped unprinted, and every line spoken in the new
    /// zone with it. `ui_chat::tests::a_zone_change_must_not_deregister_the_channel_it_renames` is
    /// that claim.
    ///
    /// Freeing and re-claiming instead would also renumber: the slot is the channel's `/N`.
    ///
    /// Returns the slot number when there was one to rename.
    pub(crate) fn rename_slot(&mut self, old: &str, new: &str) -> Option<u32> {
        let n = self.number_of(old)?;
        let slot = self.joined[n as usize - 1].as_mut()?;
        slot.name = new.to_string();
        // `0x49bcd3`: `(old == 3) ? 1 : 2`. A suspended slot comes back as a plain pending join —
        // our `Joined` is that state 1 — so its confirming notice reads `YOU_JOINED`, not
        // `YOU_CHANGED`; any other state is a rename awaiting its re-join, and the notice resolves
        // it.
        slot.state = if slot.state == SlotState::Suspended {
            SlotState::Joined
        } else {
            SlotState::Renamed
        };
        Some(n)
    }

    /// **Suspend the slot holding `name` — the row stopped applying** (`0x49bcf0`, state 3).
    ///
    /// Walking out of a capital with `Trade` joined is the only case the 1.12 data produces. The
    /// LEAVE has already gone out; this is what keeps the record, so the notice that comes back is
    /// `SUSPENDED` rather than `YOU_LEFT` and the window keeps its registration
    /// ([`ChannelSlot::suspended`]). Freeing it instead is the same bug the rename fixes, on the
    /// one row a rename cannot reach.
    pub(crate) fn suspend_slot(&mut self, name: &str) -> Option<u32> {
        let n = self.number_of(name)?;
        self.joined[n as usize - 1].as_mut()?.state = SlotState::Suspended;
        Some(n)
    }

    /// **The confirming `YOU_JOINED` resolves the slot's state** — the reference's `0x49bb20`
    /// writes `+0x9c = 0` unconditionally on that arm. Without this a renamed slot would stay in
    /// [`SlotState::Renamed`] for good and every later join notice would read "Changed Channel".
    ///
    /// Separate from [`Self::claim_slot`] because the notice arrives for slots we already number —
    /// a rename is exactly that case, and it is the one that must not take a second slot.
    pub(crate) fn confirm_slot(&mut self, name: &str) {
        if let Some(n) = self.number_of(name) {
            if let Some(slot) = self.joined[n as usize - 1].as_mut() {
                slot.state = SlotState::Joined;
            }
        }
    }

    /// The state of the slot holding `name` — the notice arms' own split. A name we hold no slot
    /// for answers `None`, which is the leg where the reference defaults every derived arg.
    pub(crate) fn slot_state(&self, name: &str) -> Option<SlotState> {
        self.number_of(name)
            .and_then(|n| self.joined[n as usize - 1].as_ref())
            .map(|s| s.state)
    }

    /// The roster as the VM's mirror wants it — `GetChannelName`/`GetChannelList` read names, and
    /// the holes have to survive the trip or `/N` addresses the wrong channel.
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

    /// Give `name` a slot: **the first free one**, else a new one while under [`MAX_CHANNELS`] —
    /// the reference's allocator `0x49b980` (see [`ChannelState`]). Already-joined answers its own
    /// number rather than taking a second slot. `None` = all ten are taken.
    ///
    /// The reference also prints a chat error when full (`0x49b9c5: push 0x199` → `0x496720`); we
    /// decline the join and warn instead — one line of feedback we cannot quote without the
    /// error-string table this build indexes by id, and the structural half is what matters.
    pub(crate) fn claim_slot(&mut self, name: &str) -> Option<u32> {
        if let Some(n) = self.number_of(name) {
            // A confirmed join on a slot we already hold clears its suspension — the reference's
            // `0x49bb20` writes state 0 unconditionally. This is the other half of the state-3
            // bypass: walking back into a capital re-joins `Trade - City` under the name the slot
            // already carries.
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

    /// Free the slot holding `name` — **cleared in place** (`0x49bbd0`), so every other channel
    /// keeps its number. Answers the number that just went empty.
    pub(crate) fn free_slot(&mut self, name: &str) -> Option<u32> {
        let n = self.number_of(name)?;
        self.joined[n as usize - 1] = None;
        Some(n)
    }

    /// Fill an event's four channel slots (arg4, arg7, arg8, arg9) in place.
    ///
    /// **They are one record, not four fields.** In the reference all four are read off the
    /// client's local channel record — `slot+0x00`, `+0x04`, `+0x94`, `+0x98` — so a name that is
    /// *not* in the local list has no record to read and every one of them is empty: arg4 falls
    /// back to the bare incoming name and arg7/arg8/arg9/arg10 are `0/0/""/0` together. They are
    /// never independently populated. (`0x49b12f` installs the empty defaults together; the
    /// `"%d. %s"` prefix at `0x8445c8` is applied on the hit leg `0x49aa48`, and `0x49aa86` is the
    /// bare-name miss leg.)
    ///
    /// So: on entry `event.channel` holds the name as the wire gave it ("General - Elwynn Forest").
    /// If we are in that channel, on exit arg4 is the numbered display form, arg9 the stored name
    /// **with its " - Zone" tail intact** (`0x49a4ea`: the DBC name column *is* the format string
    /// the client built the stored name with), arg8 the 1-based local slot and arg7 the
    /// `ChatChannels.dbc` ChannelID — 0 for a custom channel. If we are not, nothing is stamped.
    ///
    /// arg7 is resolved from the name against `ChatChannels.dbc` rather than remembered per join.
    /// That is safe *because* it only ever runs on the hit leg: the id the client stores in
    /// `slot+0x94` came from the same DBC row at join time, and vmangos resolves the name the same
    /// way (`GetChannelEntryFor`), so no two of the three can disagree.
    pub(crate) fn stamp_channel(&self, event: &mut super::event::ChatEvent) {
        // A miss leaves all four alone — see the "one record" note above.
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
