//! The reference callbacks that signal interface events from the object packets: the field watches
//! on units and the active player, and the packet handlers that signal unit events through the
//! token fan-out `0x515e50`. The packets write one [`FieldNotice`] per signal, in wire order, and
//! the unit feed fires each (`crate::ui_unit`), so an event follows the change that raised it and
//! nothing else: not a login, not a `/reload`, not a value read for the first time, not a token
//! that comes to name another unit.
//!
//! A block's notify runs each watch whose bytes now differ from the copy its object's last write
//! in the packet took, once, at the first field of its range the block visits (`0x465570` sets the
//! record's countdown to 1; `repz cmpsb` at `0x4655bb`), so the notices of one block come out in
//! field order ([`Notified`]). At one field the notify pass splices the type watchers' list ahead
//! of the object's own (`0x46544a`, `0x465463`), each appended at the pending list's tail
//! (`0x465a14`) behind the watches a lower field already spliced. The notify pass `0x465330` has
//! two callers, a values block (`0x465277`) and a create of an object already shown (`0x465d8e`);
//! a first create runs none (`0x465d80 je 0x465d98`), and a `/reload` receives no update at all.
//!
//! The watches here: the unit-field bridge `0x51bbb0` on every unit with token flags; the unit
//! type watchers `0x6041f0` and the player type watchers `0x5e2510` whose callbacks signal; the
//! active player's `0x5de4b0`, rest-state and charmed-by watches registered in `0x5dd8a0`, its
//! skill watches `0x5de180` and `0x5de470`, its ammunition watch `0x4c70b0`, and the weapon arms
//! of the visible-item watch `0x5e2810`. The item watchers' signals are [`crate::item_watch`]'s.

use benilla_protocol::messages::ObjectType;
use benilla_protocol::{ObjectFields, OwnerFallback};
use bevy::prelude::*;

use crate::tutorial::id as tutorial;

/// One signal a reference callback raised, in the order the packets raised them.
#[derive(Message, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FieldNotice {
    /// The unit-field bridge `0x51bd50`: event `id`, the watched field's byte offset in the unit
    /// section over 4 (`0x51bd55`), or the fixed id of a player-block watch (`0x51bd90`), through
    /// `0x515e50` once per token naming `guid`. Only a unit with token flags carries the bridge.
    Unit { guid: u64, id: u16 },
    /// Any other callback's own `0x515e50(guid, id)`: a type watcher's, a packet handler's or a
    /// cache answer's. It reaches every token naming `guid`, with no watcher of the unit's own.
    Signal { guid: u64, id: u16 },
    /// A callback's `FrameScript_SignalEvent2(id, "%s", "player")`: `0x5de4b0` on the active
    /// player, the event its registration pushes as the argument (`0x5de4b3`, `0x5de4c1`); the
    /// ammunition watch's, the item slot path's and the ammo getters' record callback's
    /// `UNIT_INVENTORY_CHANGED` (`0x4c7110`, `0x4c72e0`, `0x4c8509`).
    Player { id: u16 },
    /// `0x5de4e0` on `PLAYER_BYTES_2` byte 3: `UPDATE_EXHAUSTION` bare (`0x5de4e8`), then the
    /// rest-state line for the new state.
    RestState { old: u8, new: u8 },
    /// `0x5ed990`: `UNIT_ATTACK`, `UNIT_DEFENSE` and `UNIT_RANGED_ATTACK_POWER`, each with
    /// `"player"`, from the skill value watch `0x5de180` (`0x5de214`) or the temporary-bonus watch
    /// `0x5de470` (`0x5de495`).
    CombatSkill,
    /// The weapon arms of `0x5dee30` for the active player (`0x5deeae`): main hand (15) and off
    /// hand (16) signal `UNIT_ATTACK` through `0x5ed970`, ranged (17) `UNIT_RANGED_ATTACK_POWER`
    /// (`0x5deecb`).
    Weapon { slot: u8 },
    /// A callback's `FrameScript_SignalEvent(id)` (`0x703e50`): the event with no arguments, for
    /// every listener, whatever unit raised it.
    Global { id: u16 },
    /// The quest log's watcher `0x4de4d0` on the active player, registered at world entry on the
    /// 0xf0 bytes at the player section's `+0x28` (`0x4de43b`-`0x4de456`): the rebuild and its
    /// `QUEST_LOG_UPDATE`.
    QuestLog,
    /// `0x5ee990`'s PvP-preference arm on the active player (`0x5eeaff`): the toast `0x1b5` when
    /// the bit is now set, else `0x1b6` (`0x496720`), then `PVP_TOGGLE_ON_VERBOSE` or
    /// `PVP_TOGGLE_OFF_VERBOSE` (`0x8606e0`, `0x8606c8`) as a chat line of type `0xa`
    /// (`0x5eeb5d`).
    PvpPreference { on: bool },
    /// The drunk watcher `0x5e2a90` on any player's `PLAYER_BYTES_3` byte 1: the byte's copy and
    /// its value now, from which the run signals and speaks (`crate::ui_unit::field_events`).
    Drunk { guid: u64, old: u8, new: u8 },
    /// `FireTutorial(id, 0)` (`0x4b5390`) from a watcher on the active player: the number's
    /// fire-once bit tested and set, then its `TutorialPopup` cue and `TUTORIAL_TRIGGER` at once,
    /// in the watcher's order.
    Tutorial { id: u32 },
}

/// The event ids the producers pass to `0x515e50`, by their event-table names.
pub(crate) mod id {
    pub(crate) const UNIT_PET: u16 = 0;
    pub(crate) const UNIT_HEALTH: u16 = 16;
    /// `UNIT_MANA`; a power event is this plus the power type (`add edx,0x11`).
    pub(crate) const POWER: u16 = 17;
    pub(crate) const UNIT_MAXHEALTH: u16 = 22;
    /// `UNIT_MAXMANA`; a maximum-power event is this plus the power type (`add edx,0x17`).
    pub(crate) const MAX_POWER: u16 = 23;
    pub(crate) const UNIT_LEVEL: u16 = 28;
    pub(crate) const UNIT_FACTION: u16 = 29;
    pub(crate) const UNIT_DISPLAYPOWER: u16 = 30;
    pub(crate) const UNIT_AURA: u16 = 41;
    pub(crate) const UNIT_NAME_UPDATE: u16 = 183;
    pub(crate) const UNIT_INVENTORY_CHANGED: u16 = 186;
    pub(crate) const UNIT_CLASSIFICATION_CHANGED: u16 = 187;
    pub(crate) const PLAYER_FLAGS_CHANGED: u16 = 407;
    pub(crate) const UNIT_QUEST_LOG_CHANGED: u16 = 522;
    pub(crate) const PLAYER_REGEN_DISABLED: u16 = 190;
    pub(crate) const PLAYER_REGEN_ENABLED: u16 = 191;
    pub(crate) const PET_ATTACK_START: u16 = 334;
    pub(crate) const PET_ATTACK_STOP: u16 = 335;
    pub(crate) const PLAYER_GUILD_UPDATE: u16 = 345;
    pub(crate) const TABARD_CANSAVE_CHANGED: u16 = 376;
    pub(crate) const PLAYER_UPDATE_RESTING: u16 = 405;
    pub(crate) const PET_UI_UPDATE: u16 = 427;
    pub(crate) const PLAYTIME_CHANGED: u16 = 530;
}

/// `CGPlayer_C::RefreshGuildTag` (`0x5e08e0`) on the player `guid`: `TABARD_CANSAVE_CHANGED` bare
/// (`0x5e0907`), then `PLAYER_GUILD_UPDATE` through `0x515e50` (`0x5e0922`). A player's first create
/// runs it from `PostInit` (`0x5dec19`) and a `PLAYER_GUILDID` change from `0x5e2795`.
pub(crate) fn guild_tag(guid: u64) -> [FieldNotice; 2] {
    [
        FieldNotice::Global {
            id: id::TABARD_CANSAVE_CHANGED,
        },
        FieldNotice::Signal {
            guid,
            id: id::PLAYER_GUILD_UPDATE,
        },
    ]
}

/// The player type watcher `0x5e2890` on `PLAYER_GUILD_TIMESTAMP` (player section `+0x24`,
/// registered at `0x5e25ef`): when it runs, the player's guild id and its new stamp (`0x5e28c1`,
/// `0x5e28c4`), for the guild cache to weigh.
pub(crate) fn guild_timestamp(
    block: &Notified,
    kind: ObjectType,
    fields: &ObjectFields,
) -> Option<(u32, u32)> {
    (kind == ObjectType::Player
        && block
            .fired(PLAYER_SECTION, GUILD_TIMESTAMP.0, GUILD_TIMESTAMP.1)
            .is_some())
    .then(|| (fields.player_guild_id(), fields.player_guild_timestamp()))
}

/// The first field of the object, unit and player sections: their byte offsets in the field
/// array, 0, 0x18 and 0x2f0 (`0x465690`), over 4.
const OBJECT_SECTION: u16 = 0;
const UNIT_SECTION: u16 = 6;
const PLAYER_SECTION: u16 = 188;

/// The bridge ids: every unit field below `0xb6` (`0x51bc1d`) whose name in the event table
/// `0xbe1198` is set (`0x51bbc3`), each one watch of [`bridge_len`] bytes. `UNIT_HAPPINESS` (21)
/// is left out: the pet stats feed fires it under its own `Deviation:`.
pub(crate) const BRIDGE_IDS: [u16; 53] = [
    0, 2, 16, 17, 18, 19, 20, 22, 23, 24, 25, 26, 27, 28, 29, 30, 40, 41, 107, 120, 121, 122, 128,
    129, 130, 131, 132, 135, 136, 137, 143, 144, 145, 146, 147, 148, 149, 150, 151, 152, 153, 154,
    155, 159, 160, 161, 162, 163, 164, 165, 166, 167, 174,
];

/// A bridge watch's length: the selector byte at `0x51bc98 + id`, through the jump table
/// `0x51bc84`, picks 8 (`0x51bbdd`), 0xd8 (`0x51bbe4`), 0x30 (`0x51bbeb`) or 0x1c (`0x51bbf2`);
/// every other id, and any past 0xae (`0x51bbc7`), is 4 (`0x51bbf9`).
pub(crate) fn bridge_len(id: u16) -> u16 {
    match id {
        0 | 2 | 4 | 10 => 8,
        41 => 0xd8,
        107 => 0x30,
        167 | 174 => 0x1c,
        _ => 4,
    }
}

/// The bridge's two player-block watches, registered on a unit whose type mask has the player
/// bit (`0x51bc2b`-`0x51bc31`) in this order: the 19 visible items, 0x390 bytes at the player
/// section's `+0x118` (`0x51bc3c`), signalling `UNIT_INVENTORY_CHANGED` through `0x51bd70`
/// (`0x51bd73`); the quest log, 0xf0 bytes at `+0x28` (`0x51bc63`), signalling `0x20a` through
/// `0x51bd90` (`0x51bd93`).
const VISIBLE_ITEMS_WATCH: (u16, u16) = (0x118, 0x390);
const QUEST_LOG_WATCH: (u16, u16) = (0x28, 0xf0);

/// The active player's ammunition watch `0x4c70b0`, `PLAYER_AMMO_ID` at the player section's
/// `+0x102c`, registered at world entry (`0x4c6fea`, from `0x49097e`).
const AMMO_ID: (u16, u16) = (0x102c, 4);

/// `0x5de4b0`'s registrations in `0x5dd8a0`, in registration order: the byte offset in the player
/// section, the length, and the event id pushed as the argument.
const PLAYER_EVENT_WATCHES: [(u16, u16, u16); 11] = [
    // `UPDATE_EXHAUSTION`, the rest pool (`0x5ddb24`).
    (0xf6c, 4, 406),
    // `PLAYER_XP_UPDATE`, the XP and the next level's (`0x5ddb4b`, `0x5ddb72`).
    (0x840, 4, 189),
    (0x844, 4, 189),
    // `UNIT_RESISTANCES`, both resistance buff-mod arrays as one watch (`0x5ddb99`).
    (0xf9c, 0x38, 149),
    // `PLAYER_DAMAGE_DONE_MODS`, the positive, negative and percent arrays (`0x5ddbc0`,
    // `0x5ddbe7`, `0x5ddc0e`).
    (0xfd4, 0x1c, 201),
    (0xff0, 0x1c, 201),
    (0x100c, 0x1c, 201),
    // `PLAYER_PVP_KILLS_CHANGED`, the session kills (`0x5ddc35`).
    (0x1098, 4, 523),
    // `PLAYER_PVP_RANK_CHANGED`, the whole `PLAYER_BYTES_3` and `PLAYER_FIELD_BYTES2` byte 0
    // (`0x5ddc5c`, `0x5ddc83`).
    (0x1c, 4, 524),
    (0x10c0, 1, 524),
    // `UPDATE_FACTION`, the watched faction index (`0x5ddcce`).
    (0x10c4, 4, 372),
];

/// `0x5de4e0`'s range, `PLAYER_BYTES_2` byte 3 (`0x5ddb0b`).
const REST_STATE: u16 = 0x1b;

/// The skill block: entry `k` at `0x848 + 12k` of the player section, 128 entries (`0x5dda54` to
/// `0x5ddaf0`). The value watch covers the value word at `+4` and the temporary-bonus watch the
/// word at `+8`, two bytes each.
const SKILL_VALUE: u16 = 0x84c;
const SKILL_TEMP_BONUS: u16 = 0x850;
const SKILL_STRIDE: u16 = 12;
const SKILLS: u16 = 128;

/// The visible-item watch's ranges, `0x118 + 0x30·slot` of the player section, 0x30 bytes
/// (`0x5e2541`), for the three slots `0x5dee30` signals on.
const VISIBLE_ITEM: u16 = 0x118;
const VISIBLE_STRIDE: u16 = 0x30;
const WEAPON_SLOTS: [u8; 3] = [15, 16, 17];

/// The unit type watchers of `0x6041f0` whose callbacks signal, as (byte offset, length) in the
/// unit section: `UNIT_FIELD_FLAGS` (`0x6042e2`, `0x6044b0` → `0x5ff580`), the faction template
/// (`0x60434a`, `0x6048d0`), `CHARMEDBY` and `SUMMONEDBY` (`0x604364`, `0x604910`), `PERSUADED`
/// (`0x60437e`, `0x604950`), the pet number (`0x604468`, `0x604bf0`) and the dynamic flags
/// (`0x60441a`, `0x604b30` → `0x600440`); and in the object section `OBJECT_FIELD_ENTRY`
/// (`0x60427d`, `0x604600`).
const UNIT_FLAGS: (u16, u16) = (0xa0, 4);
const FACTION_TEMPLATE: (u16, u16) = (0x74, 4);
const OWNERS: (u16, u16) = (0x10, 0x10);
const PERSUADED: (u16, u16) = (0x30, 8);
const PET_NUMBER: (u16, u16) = (0x214, 4);
const DYNAMIC_FLAGS: (u16, u16) = (0x224, 4);
const ENTRY: (u16, u16) = (0xc, 4);

/// The player type watchers of `0x5e2510` whose callbacks signal, as (byte offset, length) in the
/// player section: `PLAYER_FLAGS` (`0x5e2850` → `0x5ee990`), `PLAYER_GUILDID` (`0x5e258f` →
/// `0x5e2770`), `PLAYER_GUILDRANK` (`0x5e25a7` → `0x5e27b0`), `PLAYER_BYTES_3` bytes 2, 3 and 1
/// in that order (`0x5e2a30`, `0x5e2a50`, `0x5e2a90`) and the duel team (`0x5e27d0`); and
/// `PLAYER_GUILD_TIMESTAMP` (`0x5e25ef` → `0x5e2890`), which acts on the guild cache
/// ([`guild_timestamp`]).
const PLAYER_FLAGS: (u16, u16) = (0x08, 4);
const GUILD_ID: (u16, u16) = (0x0c, 4);
const GUILD_RANK: (u16, u16) = (0x10, 4);
const BYTES_3_BYTE_1: (u16, u16) = (0x1d, 1);
const BYTES_3_BYTE_2: (u16, u16) = (0x1e, 1);
const BYTES_3_BYTE_3: (u16, u16) = (0x1f, 1);
const DUEL_TEAM: (u16, u16) = (0x20, 4);
const GUILD_TIMESTAMP: (u16, u16) = (0x24, 4);

/// The active player's own `CHARMEDBY` watch, `0x5de090` (`0x5dd9f4`): 8 bytes at the unit
/// section's `+0x10`, refreshing its reaction (`0x5de0b5`).
const CHARMED_BY: (u16, u16) = (0x10, 8);

/// Which list a watch sits on: at one field the type watchers run ahead of the object's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum List {
    Type,
    Object,
}

/// What the watches read of the world around the block's object.
pub(crate) struct Around<'a> {
    /// The active player's guid.
    pub(crate) me: Option<u64>,
    /// The creature cache's lookup with a callback (`0x60463f`): whether it holds `entry`'s
    /// record, which `0x604600` answers with the name signal at once; a miss asks for it and waits
    /// for the answer (`crate::names`).
    pub(crate) creature_lookup: &'a dyn Fn(u32) -> bool,
    /// `CGUnit_C::RefreshReaction`'s sweep for the active player ([`reaction_sweep`]).
    pub(crate) others: &'a dyn Fn() -> Vec<u64>,
    /// The pet bar's guid, the client's `[0xb714a0]`; 0 for no bar.
    pub(crate) pet_bar: u64,
}

/// One block as the notify pass reads it again (`0x465330`): the fields it visits against the dwords
/// its object's last write in the packet moved, `(index, copy, now)` ascending. Every write of a
/// held object re-copies each of its watched ranges first (`0x46664b`-`0x4666b6`, over every field
/// below the count, sent or not), so an earlier block of the packet compares against the copy the
/// last write took.
#[derive(Clone, Copy)]
pub(crate) struct Notified<'a> {
    sent: Sent<'a>,
    moved: &'a [(u16, u32, u32)],
}

/// The fields a block's notify visits: it runs the pending watches at each (`0x4654a4`).
#[derive(Clone, Copy)]
pub(crate) enum Sent<'a> {
    /// The set bits of a values block's mask, ascending.
    Fields(&'a [u16]),
    /// Every field, set or clear: the create notify of an object already shown (`dl = 1`,
    /// `0x465d8a`).
    All,
}

impl<'a> Notified<'a> {
    pub(crate) fn new(sent: Sent<'a>, moved: &'a [(u16, u32, u32)]) -> Self {
        Self { sent, moved }
    }

    /// Whether nothing a watch could compare moved.
    pub(crate) fn is_quiet(&self) -> bool {
        self.moved.is_empty()
    }

    /// Whether the block visits field `index`.
    fn visits(&self, index: u16) -> bool {
        match self.sent {
            Sent::All => true,
            Sent::Fields(sent) => sent.binary_search(&index).is_ok(),
        }
    }

    /// The dword `index` in the copy and now, when it moved.
    pub(crate) fn edge(&self, index: u16) -> Option<(u32, u32)> {
        self.moved
            .binary_search_by_key(&index, |&(i, _, _)| i)
            .ok()
            .map(|at| (self.moved[at].1, self.moved[at].2))
    }

    /// The bits of dword `index` that moved.
    fn flipped(&self, index: u16) -> u32 {
        self.edge(index).map_or(0, |(old, new)| old ^ new)
    }

    /// The field a watch of `len` bytes at byte `offset` of the section starting at field `base`
    /// runs at: the first field of its range the block visits, when any byte of the range differs
    /// from the copy (the watch's `repz cmpsb`, `0x4655bb`).
    fn fired(&self, base: u16, offset: u16, len: u16) -> Option<u16> {
        let (start, end) = (u32::from(offset), u32::from(offset) + u32::from(len));
        let below = |n: u32| {
            if n >= 4 {
                u32::MAX
            } else {
                (1u32 << (8 * n)) - 1
            }
        };
        let mut first = None;
        let mut moved = false;
        let mut at = start & !3;
        while at < end {
            let field = base + (at / 4) as u16;
            if first.is_none() && self.visits(field) {
                first = Some(field);
            }
            if let Some((old, new)) = self.edge(field) {
                let mask = below(end.min(at + 4) - at) & !below(start.max(at) - at);
                moved |= (old ^ new) & mask != 0;
            }
            at += 4;
        }
        first.filter(|_| moved)
    }

    /// Whether a watch over the whole dwords of `range` runs: the block visits one of them and
    /// one of them moved.
    pub(crate) fn runs(&self, range: std::ops::Range<u16>) -> bool {
        self.moved.iter().any(|(i, _, _)| range.contains(i))
            && range.into_iter().any(|i| self.visits(i))
    }

    /// The moved dwords the block visits, as `(index, copy, now)`: the edges
    /// [`crate::net::FieldChanged`] carries.
    pub(crate) fn edges(&self) -> impl Iterator<Item = (u16, u32, u32)> + '_ {
        self.moved
            .iter()
            .copied()
            .filter(|&(i, _, _)| self.visits(i))
    }
}

/// A watch's place in the notify pass: the field it ran at, its range's first field, its list.
type Place = (u16, u16, List);

/// The signals of one block in the notify pass's order. A stable sort keeps one callback's
/// signals, and the watches of one list at one field, in the order they ran.
#[derive(Default)]
struct Pass(Vec<(Place, FieldNotice)>);

impl Pass {
    fn push(&mut self, at: Place, notice: FieldNotice) {
        self.0.push((at, notice));
    }

    fn signal(&mut self, at: Place, guid: u64, id: u16) {
        if guid != 0 {
            self.push(at, FieldNotice::Signal { guid, id });
        }
    }

    /// `CGUnit_C::RefreshReaction` (`0x606e20`) on `guid`: `UNIT_FACTION` for it (`0x606e57`),
    /// for its `SUMMON` (`0x606e9d`) and its `CHARM` (`0x606edf`) when set, then, for the active
    /// player, for every other unit not its own (`0x606efb`).
    fn reaction(&mut self, at: Place, guid: u64, fields: &ObjectFields, around: &Around) {
        self.signal(at, guid, id::UNIT_FACTION);
        for pet in [fields.unit_summon(), fields.unit_charm()]
            .into_iter()
            .flatten()
        {
            self.signal(at, pet, id::UNIT_FACTION);
        }
        if around.me == Some(guid) {
            for other in (around.others)() {
                self.signal(at, other, id::UNIT_FACTION);
            }
        }
    }

    fn into_notices(mut self) -> Vec<FieldNotice> {
        self.0.sort_by_key(|&(at, _)| at);
        self.0.into_iter().map(|(_, n)| n).collect()
    }
}

/// The notices of one block's notify on `guid`, created as `kind`: `fields` is the store now.
pub(crate) fn values(
    around: &Around,
    guid: u64,
    kind: ObjectType,
    fields: &ObjectFields,
    block: &Notified,
) -> Vec<FieldNotice> {
    if block.is_quiet() || !matches!(kind, ObjectType::Unit | ObjectType::Player) {
        return Vec::new();
    }
    let at = |base: u16, (offset, len): (u16, u16), list: List| -> Option<Place> {
        block
            .fired(base, offset, len)
            .map(|fired| (fired, base + offset / 4, list))
    };
    let mut pass = Pass::default();

    // The unit type watchers (`0x6041f0`), on every unit.
    if let Some(at) = at(OBJECT_SECTION, ENTRY, List::Type) {
        // `0x604600`: a cached record names the unit at once (`0x604651`); a miss asks.
        if (around.creature_lookup)(fields.object_entry().unwrap_or(0)) {
            pass.signal(at, guid, id::UNIT_NAME_UPDATE);
        }
    }
    for range in [OWNERS, PERSUADED, FACTION_TEMPLATE] {
        if let Some(at) = at(UNIT_SECTION, range, List::Type) {
            pass.reaction(at, guid, fields, around);
        }
    }
    if let Some(at) = at(UNIT_SECTION, UNIT_FLAGS, List::Type) {
        // `0x5ff580` on the flipped bits, against the new flags.
        let flipped = block.flipped(UNIT_SECTION + UNIT_FLAGS.0 / 4);
        let flags = fields.unit_flags();
        // The active player's arm first (`0x5ff5b4`, `0x5ff5c7` → `0x5ff4e0`): the in-combat bit
        // 0x80000 signals `PLAYER_REGEN_DISABLED` when now set, else `PLAYER_REGEN_ENABLED`
        // (`0x5ff513`-`0x5ff533`).
        if around.me == Some(guid) && flipped & 0x8_0000 != 0 {
            let id = if flags & 0x8_0000 != 0 {
                id::PLAYER_REGEN_DISABLED
            } else {
                id::PLAYER_REGEN_ENABLED
            };
            pass.push(at, FieldNotice::Global { id });
        }
        // The reaction on 0x4080 (`0x5ff71c`) and again on 0x1000 (`0x5ff72b`).
        if flipped & 0x4080 != 0 {
            pass.reaction(at, guid, fields, around);
        }
        if flipped & 0x1000 != 0 {
            pass.reaction(at, guid, fields, around);
            // The PvP tutorial for the active player now flagged (`0x5ff732`-`0x5ff756`).
            if around.me == Some(guid) && flags & 0x1000 != 0 {
                pass.push(
                    at,
                    FieldNotice::Tutorial {
                        id: tutorial::PVP_COMBAT,
                    },
                );
            }
        }
        // 0x800 on a unit whose charmer, else summoner, is the active player (`0x5ff75e`-
        // `0x5ff784`): `PET_ATTACK_START` when now set, else `PET_ATTACK_STOP` (`0x5ff79f`).
        if flipped & 0x800 != 0
            && around.me.is_some()
            && fields.unit_owner(OwnerFallback::SummonedBy) == around.me
        {
            let id = if flags & 0x800 != 0 {
                id::PET_ATTACK_START
            } else {
                id::PET_ATTACK_STOP
            };
            pass.push(at, FieldNotice::Global { id });
        }
        // `UNIT_AURA` on 0x8000000 (`0x5ff7e2`).
        if flipped & 0x0800_0000 != 0 {
            pass.signal(at, guid, id::UNIT_AURA);
        }
    }
    if let Some(at) = at(UNIT_SECTION, PET_NUMBER, List::Type) {
        // `0x604bf0`: `UNIT_CLASSIFICATION_CHANGED` on any change (`0x604bfb`), then
        // `PET_UI_UPDATE` bare when the unit is the pet bar's (`0x604c03`-`0x604c16`).
        pass.signal(at, guid, id::UNIT_CLASSIFICATION_CHANGED);
        if guid == around.pet_bar {
            let id = id::PET_UI_UPDATE;
            pass.push(at, FieldNotice::Global { id });
        }
    }
    if let Some(at) = at(UNIT_SECTION, DYNAMIC_FLAGS, List::Type) {
        // `0x600440` on the flipped bits: 0x20, the dead look, signals `UNIT_HEALTH` and the power
        // event of `UNIT_FIELD_BYTES_0` byte 3 (`0x6004cd`, `0x6004f0`); 0x4, the tap,
        // `UNIT_FACTION` (`0x6005bb`).
        let flipped = block.flipped(UNIT_SECTION + DYNAMIC_FLAGS.0 / 4);
        if flipped & 0x20 != 0 {
            pass.signal(at, guid, id::UNIT_HEALTH);
            pass.signal(at, guid, id::POWER + u16::from(fields.unit_power_type()));
        }
        if flipped & 0x4 != 0 {
            pass.signal(at, guid, id::UNIT_FACTION);
        }
    }

    // The bridge (`0x51bbb0`), the object's own watches.
    for bridge in BRIDGE_IDS {
        if let Some(at) = at(UNIT_SECTION, (4 * bridge, bridge_len(bridge)), List::Object) {
            pass.push(at, FieldNotice::Unit { guid, id: bridge });
        }
    }

    if kind == ObjectType::Player {
        // The player type watchers (`0x5e2510`), on every player.
        if let Some(at) = at(PLAYER_SECTION, PLAYER_FLAGS, List::Type) {
            // `0x5ee990`: the reaction on 0x180 (`0x5eea04`), then `PLAYER_FLAGS_CHANGED` on any
            // bit (`0x5eea35`).
            let flipped = block.flipped(PLAYER_SECTION + PLAYER_FLAGS.0 / 4);
            if flipped & 0x180 != 0 {
                pass.reaction(at, guid, fields, around);
            }
            pass.signal(at, guid, id::PLAYER_FLAGS_CHANGED);
            // Below the active-player gate (`0x5eea93`-`0x5eeaae`), each arm on its own bits
            // against the new flags.
            if around.me == Some(guid) {
                let flags = fields.player_flags();
                // 0x10, the ghost state (`0x5eeab4` → `0x5de9c0`): its tutorial when now set.
                if flipped & 0x10 != 0 && flags & 0x10 != 0 {
                    pass.push(
                        at,
                        FieldNotice::Tutorial {
                            id: tutorial::DEATH,
                        },
                    );
                }
                // 0x20, resting: the rest tutorial when now set (`0x5eeae6`-`0x5eeaed`), then
                // `PLAYER_UPDATE_RESTING` on either edge (`0x5eead0`, `0x5eeaf2`).
                if flipped & 0x20 != 0 {
                    if flags & 0x20 != 0 {
                        pass.push(
                            at,
                            FieldNotice::Tutorial {
                                id: tutorial::RESTING,
                            },
                        );
                    }
                    let id = id::PLAYER_UPDATE_RESTING;
                    pass.push(at, FieldNotice::Global { id });
                }
                // 0x200, the PvP preference (`0x5eeaff`).
                if flipped & 0x200 != 0 {
                    let on = flags & 0x200 != 0;
                    pass.push(at, FieldNotice::PvpPreference { on });
                }
                // 0x3000, the two play-time limits: `PLAYTIME_CHANGED` (`0x5eeb65`, `0x5eeb6f`).
                if flipped & 0x3000 != 0 {
                    let id = id::PLAYTIME_CHANGED;
                    pass.push(at, FieldNotice::Global { id });
                }
            }
        }
        if let Some(at) = at(PLAYER_SECTION, GUILD_ID, List::Type) {
            // `0x5e2770`: the player's guild tag (`0x5e2795`).
            for notice in guild_tag(guid) {
                pass.push(at, notice);
            }
        }
        if let Some(at) = at(PLAYER_SECTION, GUILD_RANK, List::Type) {
            // `0x5e27b0`: `PLAYER_GUILD_UPDATE` bare (`0x5e27b5`).
            let id = id::PLAYER_GUILD_UPDATE;
            pass.push(at, FieldNotice::Global { id });
        }
        if let Some(at) = at(PLAYER_SECTION, BYTES_3_BYTE_2, List::Type) {
            pass.signal(at, guid, id::UNIT_NAME_UPDATE);
        }
        if let Some(at) = at(PLAYER_SECTION, BYTES_3_BYTE_3, List::Type) {
            pass.signal(at, guid, id::UNIT_NAME_UPDATE);
            pass.reaction(at, guid, fields, around);
        }
        if let Some(at) = at(PLAYER_SECTION, BYTES_3_BYTE_1, List::Type) {
            // `0x5e2a90`: the byte's copy (`0x5e2b59`) and the byte now (`0x5e2ac9`).
            let (old, new) = block.edge(at.0).unwrap_or_default();
            let byte = |v: u32| (v >> (8 * (BYTES_3_BYTE_1.0 % 4))) as u8;
            let (old, new) = (byte(old), byte(new));
            pass.push(at, FieldNotice::Drunk { guid, old, new });
        }
        if let Some(at) = at(PLAYER_SECTION, DUEL_TEAM, List::Type) {
            pass.reaction(at, guid, fields, around);
        }
        // The bridge's player-block watches.
        if let Some(at) = at(PLAYER_SECTION, VISIBLE_ITEMS_WATCH, List::Object) {
            let id = id::UNIT_INVENTORY_CHANGED;
            pass.push(at, FieldNotice::Unit { guid, id });
        }
        if let Some(at) = at(PLAYER_SECTION, QUEST_LOG_WATCH, List::Object) {
            let id = id::UNIT_QUEST_LOG_CHANGED;
            pass.push(at, FieldNotice::Unit { guid, id });
        }
    }

    if kind == ObjectType::Player && around.me == Some(guid) {
        if let Some(at) = at(UNIT_SECTION, CHARMED_BY, List::Object) {
            pass.reaction(at, guid, fields, around);
        }
        // Registered at world entry (`0x4de430`), after the bridge's watch on the same range.
        if let Some(at) = at(PLAYER_SECTION, QUEST_LOG_WATCH, List::Object) {
            pass.push(at, FieldNotice::QuestLog);
        }
        if let Some(at) = at(PLAYER_SECTION, (REST_STATE, 1), List::Object) {
            let (old, new) = block.edge(at.0).unwrap_or_default();
            let byte = |v: u32| (v >> (8 * (REST_STATE % 4))) as u8;
            let (old, new) = (byte(old), byte(new));
            pass.push(at, FieldNotice::RestState { old, new });
            // State 1's row names the rested tutorial (`0x80af50`: `0x15a`, `0x19`; `0x5de542`).
            if new == 1 {
                pass.push(
                    at,
                    FieldNotice::Tutorial {
                        id: tutorial::RESTED,
                    },
                );
            }
        }
        for (offset, len, id) in PLAYER_EVENT_WATCHES {
            if let Some(at) = at(PLAYER_SECTION, (offset, len), List::Object) {
                pass.push(at, FieldNotice::Player { id });
            }
        }
        // `0x4c70b0` sets the ammunition model (`0x4c70fc`, not built) and signals (`0x4c7110`).
        if let Some(at) = at(PLAYER_SECTION, AMMO_ID, List::Object) {
            let id = id::UNIT_INVENTORY_CHANGED;
            pass.push(at, FieldNotice::Player { id });
        }
        for k in 0..SKILLS {
            let value = SKILL_VALUE + SKILL_STRIDE * k;
            if let Some(at) = at(PLAYER_SECTION, (value, 2), List::Object) {
                let (old, _) = block.edge(at.0).unwrap_or_default();
                if skill_value_signals(fields, k, old) {
                    pass.push(at, FieldNotice::CombatSkill);
                }
            }
            let bonus = SKILL_TEMP_BONUS + SKILL_STRIDE * k;
            if let Some(at) = at(PLAYER_SECTION, (bonus, 2), List::Object) {
                pass.push(at, FieldNotice::CombatSkill);
            }
        }
        for slot in WEAPON_SLOTS {
            let offset = VISIBLE_ITEM + VISIBLE_STRIDE * u16::from(slot);
            if let Some(at) = at(PLAYER_SECTION, (offset, VISIBLE_STRIDE), List::Type) {
                pass.push(at, FieldNotice::Weapon { slot });
            }
        }
    }
    pass.into_notices()
}

/// `0x5de180`'s gate ahead of its `0x5ed990` call: the new shown value, the value plus the
/// permanent bonus when the value is above 0 (`0x5de1f8`, `0x5de207`), against the old value word
/// the watch passes (`0x5de18f`), equal skipping to the end (`0x5de20c`). `old` is the value's
/// whole dword before the block.
fn skill_value_signals(fields: &ObjectFields, k: u16, old: u32) -> bool {
    let Some(slot) = fields.player_skill(k as u8) else {
        return true;
    };
    let value = u32::from(slot.value);
    let shown = if value > 0 {
        value + u32::from(slot.perm_bonus as u16)
    } else {
        0
    };
    shown != old & 0xffff
}

/// `RefreshReaction`'s sweep for the active player `me` (`0x468380` with `0x606f20`) over the held
/// units, each a guid and its descriptor: all but `me` (`0x606f6b`) and those whose charmer, else
/// summoner, is `me` (`0x606f7c`-`0x606f9e`), in guid order.
pub(crate) fn reaction_sweep<'a>(
    me: u64,
    units: impl IntoIterator<Item = (u64, &'a ObjectFields)>,
) -> Vec<u64> {
    let mut out: Vec<u64> = units
        .into_iter()
        .filter(|&(guid, fields)| {
            guid != me && fields.unit_owner(OwnerFallback::SummonedBy) != Some(me)
        })
        .map(|(guid, _)| guid)
        .collect();
    out.sort_unstable();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: u64 = 0x07;
    const MOB: u64 = 0xF130_0000_0000_0001;

    /// Merge `delta` into `store` and run the watches on what moved, as a packet of this one
    /// values block does.
    fn block(
        store: &mut ObjectFields,
        me: Option<u64>,
        guid: u64,
        delta: &[(u16, u32)],
    ) -> Vec<FieldNotice> {
        let around = Around {
            me,
            creature_lookup: &|_| false,
            others: &Vec::new,
            pet_bar: 0,
        };
        block_around(store, &around, guid, delta)
    }

    fn player() -> ObjectFields {
        ObjectFields::from_pairs(&[]).into_created(ObjectType::Player)
    }

    /// The field of player-section byte `offset`.
    fn pf(offset: u16) -> u16 {
        PLAYER_SECTION + offset / 4
    }

    /// The bridge's event id names the field `id + 6`; each moved field is one notice, in field
    /// order, and an equal rewrite runs nothing.
    #[test]
    fn each_bridge_field_is_one_watch_in_field_order() {
        let mut mob = ObjectFields::from_pairs(&[]).into_created(ObjectType::Unit);
        // All five stats and one resistance in one block.
        let delta: Vec<(u16, u32)> = (150..156).map(|f| (f, 7)).collect();
        assert_eq!(
            block(&mut mob, Some(ME), MOB, &delta),
            (144..150)
                .map(|id| FieldNotice::Unit { guid: MOB, id })
                .collect::<Vec<_>>()
        );
        assert!(
            block(&mut mob, Some(ME), MOB, &delta).is_empty(),
            "nothing moved"
        );
        // The attack power multipliers are watched (161, 164); the stat bonus arrays are not.
        assert_eq!(
            block(
                &mut mob,
                Some(ME),
                MOB,
                &[(167, 1.1f32.to_bits()), (170, 1.2f32.to_bits())]
            ),
            [
                FieldNotice::Unit { guid: MOB, id: 161 },
                FieldNotice::Unit { guid: MOB, id: 164 }
            ]
        );
        // `UNIT_FIELD_HEALTH` is id 16, `UNIT_HAPPINESS`'s `POWER5` (id 21) no watch of the bridge.
        assert_eq!(
            block(&mut mob, Some(ME), MOB, &[(22, 90), (27, 50)]),
            [FieldNotice::Unit { guid: MOB, id: 16 }]
        );
    }

    /// The active player's own watches run for it alone, and in field order across sections.
    #[test]
    fn the_player_watches_run_for_the_active_player_alone() {
        let mut me = player();
        let mut other = player();
        let xp = pf(0x840);
        let delta = [(xp, 100), (pf(0x10c4), 3)];
        assert_eq!(
            block(&mut me, Some(ME), ME, &delta),
            [
                FieldNotice::Player { id: 189 },
                FieldNotice::Player { id: 372 }
            ]
        );
        assert!(block(&mut other, Some(ME), 0x08, &delta).is_empty());
        // A level-up moves both XP fields: two watches.
        assert_eq!(
            block(&mut me, Some(ME), ME, &[(xp, 0), (xp + 1, 400)]),
            [FieldNotice::Player { id: 189 }; 2]
        );
        // The last rested kill moves the rest byte (194), the XP (716) and the pool (1175): field
        // order, not the registration order that has the pool ahead of the XP.
        let (rest, pool) = (pf(0x1b), pf(0xf6c));
        block(&mut me, Some(ME), ME, &[(rest, 1 << 24), (pool, 30)]);
        assert_eq!(
            block(
                &mut me,
                Some(ME),
                ME,
                &[(rest, 2 << 24), (xp, 60), (pool, 0)]
            ),
            [
                FieldNotice::RestState { old: 1, new: 2 },
                FieldNotice::Player { id: 189 },
                FieldNotice::Player { id: 406 }
            ]
        );
        // A stat (unit section) before the XP (player section) in one block.
        assert_eq!(
            block(&mut me, Some(ME), ME, &[(150, 20), (xp, 10)]),
            [
                FieldNotice::Unit { guid: ME, id: 144 },
                FieldNotice::Player { id: 189 }
            ]
        );
    }

    /// The rank bar watch is one byte of `PLAYER_FIELD_BYTES2`, the rest-state watch one byte of
    /// `PLAYER_BYTES_2`: the other bytes of their dwords run neither.
    #[test]
    fn a_byte_watch_reads_its_own_byte_only() {
        let mut me = player();
        let bytes2 = pf(0x10c0);
        assert!(block(&mut me, Some(ME), ME, &[(bytes2, 0x0100)]).is_empty());
        assert_eq!(
            block(&mut me, Some(ME), ME, &[(bytes2, 0x0105)]),
            [FieldNotice::Player { id: 524 }]
        );
        let rest = pf(0x1b);
        assert!(block(&mut me, Some(ME), ME, &[(rest, 0x00ff_ffff)]).is_empty());
        assert_eq!(
            block(&mut me, Some(ME), ME, &[(rest, 0x02ff_ffff)]),
            [FieldNotice::RestState { old: 0, new: 2 }]
        );
        // The whole `PLAYER_BYTES_3` is the rank watch: its gender byte runs it.
        assert_eq!(
            block(&mut me, Some(ME), ME, &[(pf(0x1c), 1)]),
            [FieldNotice::Player { id: 524 }]
        );
    }

    /// The drunk watcher `0x5e2a90` is a player type watcher on `PLAYER_BYTES_3` byte 1
    /// (`0x5e2637`-`0x5e2648`), so it runs on every player, after the byte 2 and byte 3 watchers
    /// registered ahead of it (`0x5e2618`, `0x5e2630`), with the byte's copy (`0x5e2b59`) and the
    /// byte now (`0x5e2ac9`). The other bytes of the dword do not run it. On the active player
    /// the rank watch on the whole dword runs after it, the object's own list.
    #[test]
    fn the_drunk_watcher_runs_on_every_player_with_its_old_byte() {
        const OTHER: u64 = 0x08;
        let bytes_3 = pf(0x1c);
        let mut other = player();
        assert_eq!(
            block(&mut other, Some(ME), OTHER, &[(bytes_3, 60 << 8)]),
            [FieldNotice::Drunk {
                guid: OTHER,
                old: 0,
                new: 60
            }]
        );
        assert!(
            block(&mut other, Some(ME), OTHER, &[(bytes_3, 60 << 8 | 1)]).is_empty(),
            "the gender byte alone"
        );
        assert_eq!(
            block(&mut other, Some(ME), OTHER, &[(bytes_3, 0x0102_0001)]),
            [
                signal(OTHER, id::UNIT_NAME_UPDATE),
                signal(OTHER, id::UNIT_NAME_UPDATE),
                signal(OTHER, id::UNIT_FACTION),
                FieldNotice::Drunk {
                    guid: OTHER,
                    old: 60,
                    new: 0
                }
            ],
            "bytes 2, 3 and 1, in registration order"
        );
        let mut me = player();
        assert_eq!(
            block(&mut me, Some(ME), ME, &[(bytes_3, 90 << 8)]),
            [
                FieldNotice::Drunk {
                    guid: ME,
                    old: 0,
                    new: 90
                },
                FieldNotice::Player { id: 524 }
            ]
        );
    }

    /// A multi-dword watch runs once however many of its dwords moved; the stat bonus arrays
    /// between the coinage and the resistance mods have no watch.
    #[test]
    fn a_wide_watch_runs_once_per_block() {
        let mut me = player();
        let mods = pf(0xf9c);
        assert_eq!(
            block(
                &mut me,
                Some(ME),
                ME,
                &[(mods, 5), (mods + 3, 9), (mods + 13, 2)]
            ),
            [FieldNotice::Player { id: 149 }]
        );
        // `PLAYER_FIELD_POSSTAT0` .. `NEGSTAT4`, fields 1177-1186.
        let bonus: Vec<(u16, u32)> = (1177..1187).map(|f| (f, 3)).collect();
        assert!(block(&mut me, Some(ME), ME, &bonus).is_empty());
    }

    /// The value watch signals when the shown value differs from the old value word; the
    /// temporary-bonus watch on any change; the maximum and permanent bonus run neither.
    #[test]
    fn the_skill_watches_reach_the_combat_signals() {
        let mut me = player();
        let value = pf(SKILL_VALUE);
        let bonus = pf(SKILL_TEMP_BONUS);
        // Skill 95 (Defense) in entry 0 at 1/300.
        block(
            &mut me,
            Some(ME),
            ME,
            &[(value - 1, 95), (value, (300 << 16) | 1)],
        );
        assert_eq!(
            block(&mut me, Some(ME), ME, &[(value, (300 << 16) | 2)]),
            [FieldNotice::CombatSkill]
        );
        // The maximum alone (`0x5de450`), and the permanent bonus alone, signal nothing.
        assert!(block(&mut me, Some(ME), ME, &[(value, (305 << 16) | 2)]).is_empty());
        assert!(block(&mut me, Some(ME), ME, &[(bonus, 5 << 16)]).is_empty());
        // The temporary bonus, in entry 1.
        assert_eq!(
            block(&mut me, Some(ME), ME, &[(bonus + 3, 10)]),
            [FieldNotice::CombatSkill]
        );
        // With a permanent bonus of 5: a value of 2 → 7 shows 12 against the old word 2, and a
        // drop of exactly the bonus, 7 → 2, shows 7, the old word itself, so the gate skips it.
        assert_eq!(
            block(&mut me, Some(ME), ME, &[(value, (305 << 16) | 7)]),
            [FieldNotice::CombatSkill]
        );
        assert!(block(&mut me, Some(ME), ME, &[(value, (305 << 16) | 2)]).is_empty());
    }

    /// The visible-item watch signals on the three weapon slots only, on any of their 12 dwords.
    #[test]
    fn the_weapon_slots_signal_on_any_dword() {
        let mut me = player();
        let slot = |n: u16| pf(VISIBLE_ITEM + VISIBLE_STRIDE * n);
        assert_eq!(
            block(
                &mut me,
                Some(ME),
                ME,
                &[(slot(15) + 2, 2504), (slot(16) + 5, 1), (slot(17) + 11, 3)]
            ),
            [
                // The bridge's watch over all 19 slots, spliced at their first field ahead.
                bridge(ME, id::UNIT_INVENTORY_CHANGED),
                FieldNotice::Weapon { slot: 15 },
                FieldNotice::Weapon { slot: 16 },
                FieldNotice::Weapon { slot: 17 }
            ]
        );
        // The chest (slot 4) and a ring (10) do not reach it, only the bridge's.
        assert_eq!(
            block(
                &mut me,
                Some(ME),
                ME,
                &[(slot(4) + 2, 1), (slot(10) + 2, 1)]
            ),
            [bridge(ME, id::UNIT_INVENTORY_CHANGED)]
        );
    }

    /// The bridge's visible-item watch (`0x51bc3c`): one over the 19 slots' 0x390 bytes at the
    /// player section's `+0x118`, on every player, signalling `0xba` (`0x51bd73`) once a block.
    #[test]
    fn the_visible_item_bridge_runs_on_every_player_once_a_block() {
        let mut other = player();
        let first = pf(VISIBLE_ITEM);
        let last = pf(VISIBLE_ITEM + 0x390 - 4);
        assert_eq!(
            block(&mut other, Some(ME), OTHER, &[(first + 2, 7366), (last, 1)]),
            [bridge(OTHER, id::UNIT_INVENTORY_CHANGED)]
        );
        assert!(
            block(&mut other, Some(ME), OTHER, &[(last + 1, 1)]).is_empty(),
            "the inventory seats past it"
        );
        let mut mob = unit();
        assert!(
            block(&mut mob, Some(ME), MOB, &[(first + 2, 7366)]).is_empty(),
            "a creature has no player block watch"
        );
    }

    /// The ammunition watch `0x4c70b0`, registered on the active player alone at world entry
    /// (`0x4c6fea`, from `0x49097e`): `PLAYER_AMMO_ID` signals `UNIT_INVENTORY_CHANGED` with
    /// `"player"` (`0x4c7110`).
    #[test]
    fn the_ammo_watch_signals_the_active_player_alone() {
        let ammo = pf(0x102c);
        let mut me = player();
        assert_eq!(
            block(&mut me, Some(ME), ME, &[(ammo, 2512)]),
            [FieldNotice::Player {
                id: id::UNIT_INVENTORY_CHANGED
            }]
        );
        assert!(block(&mut me, Some(ME), ME, &[(ammo, 2512)]).is_empty());
        let mut other = player();
        assert!(block(&mut other, Some(ME), OTHER, &[(ammo, 2512)]).is_empty());
    }

    const PET: u64 = 0xF140_0000_0000_0077;
    const OTHER: u64 = 0xF130_0000_0000_0009;

    fn unit() -> ObjectFields {
        ObjectFields::from_pairs(&[]).into_created(ObjectType::Unit)
    }

    /// [`block`] with the world's answers given: the creature cache's and the reaction sweep's.
    fn block_around(
        store: &mut ObjectFields,
        around: &Around,
        guid: u64,
        delta: &[(u16, u32)],
    ) -> Vec<FieldNotice> {
        let delta = ObjectFields::from_pairs(delta);
        let sent: Vec<u16> = delta.raw_fields().map(|(i, _)| i).collect();
        let mut moved = Vec::new();
        store.merge_diff(delta, |i, o, n| moved.push((i, o, n)));
        let kind = store.created_as().expect("created");
        values(
            around,
            guid,
            kind,
            store,
            &Notified::new(Sent::Fields(&sent), &moved),
        )
    }

    fn signal(guid: u64, id: u16) -> FieldNotice {
        FieldNotice::Signal { guid, id }
    }

    fn bridge(guid: u64, id: u16) -> FieldNotice {
        FieldNotice::Unit { guid, id }
    }

    /// The wide watches are one each, run by any of their dwords: the charm and summon guids (8
    /// bytes), the aura array with its flags (0xd8), the applications (0x30), and the power-cost
    /// modifier and multiplier (0x1c each) whose id names `UNIT_MANA`.
    #[test]
    fn a_wide_bridge_watch_is_one_signal_from_any_of_its_dwords() {
        let mut mob = unit();
        assert_eq!(
            block(&mut mob, Some(ME), MOB, &[(9, 1)]),
            [bridge(MOB, 2)],
            "SUMMON's high half alone"
        );
        assert_eq!(
            block(&mut mob, Some(ME), MOB, &[(9, 0), (8, 2)]),
            [bridge(MOB, 2)],
            "both halves, one watch"
        );
        assert_eq!(
            block(&mut mob, Some(ME), MOB, &[(47, 133), (100, 3)]),
            [bridge(MOB, 41)],
            "the first aura slot and the last flag dword"
        );
        assert_eq!(
            block(&mut mob, Some(ME), MOB, &[(124, 1)]),
            [bridge(MOB, 107)]
        );
        // `UNIT_FIELD_POWER_COST_MODIFIER` 173-179 and `_MULTIPLIER` 180-186.
        assert_eq!(
            block(
                &mut mob,
                Some(ME),
                MOB,
                &[(179, 5), (180, 0.9f32.to_bits())]
            ),
            [bridge(MOB, 167), bridge(MOB, 174)]
        );
        assert_eq!(
            block(&mut mob, Some(ME), MOB, &[(176, 2)]),
            [bridge(MOB, 167)]
        );
        // `UNIT_FIELD_CHARMEDBY` (id 4) is 8 bytes in the selector but has no name, so no bridge
        // watch: the reaction of the owner type watcher alone, naming the summon set above.
        assert_eq!(
            block(&mut mob, Some(ME), MOB, &[(10, 7)]),
            [signal(MOB, 29), signal(2, 29)]
        );
    }

    /// At one field the type watcher runs ahead of the bridge: `0x5ff580`'s `UNIT_AURA` on bit 27
    /// before `UNIT_FLAGS`; its reaction on the PvP bit names the unit, then its summon and charm.
    #[test]
    fn the_flags_type_watcher_runs_ahead_of_the_bridge() {
        let mut mob = ObjectFields::from_pairs(&[(8, PET as u32), (9, (PET >> 32) as u32)])
            .into_created(ObjectType::Unit);
        assert_eq!(
            block(&mut mob, Some(ME), MOB, &[(46, 0x0800_0000)]),
            [signal(MOB, 41), bridge(MOB, 40)]
        );
        assert_eq!(
            block(&mut mob, Some(ME), MOB, &[(46, 0x0800_1000)]),
            [signal(MOB, 29), signal(PET, 29), bridge(MOB, 40)]
        );
        // 0x80 and 0x1000 together: two refreshes (`0x5ff71c`, `0x5ff72b`).
        assert_eq!(
            block(&mut mob, Some(ME), MOB, &[(46, 0x0800_0080)]),
            [
                signal(MOB, 29),
                signal(PET, 29),
                signal(MOB, 29),
                signal(PET, 29),
                bridge(MOB, 40)
            ]
        );
        // A bit no arm tests: the bridge alone.
        assert_eq!(
            block(&mut mob, Some(ME), MOB, &[(46, 0x0800_0088)]),
            [bridge(MOB, 40)]
        );
    }

    /// The active player's reaction sweeps every other unit the sweep hands it, after its own pet.
    #[test]
    fn the_players_reaction_sweeps_the_other_units() {
        let mut me = ObjectFields::from_pairs(&[(8, PET as u32), (9, (PET >> 32) as u32)])
            .into_created(ObjectType::Player);
        let around = Around {
            me: Some(ME),
            creature_lookup: &|_| false,
            others: &|| vec![MOB, OTHER],
            pet_bar: 0,
        };
        assert_eq!(
            block_around(&mut me, &around, ME, &[(35, 2)]),
            [
                signal(ME, 29),
                signal(PET, 29),
                signal(MOB, 29),
                signal(OTHER, 29),
                bridge(ME, 29)
            ]
        );
        // Another unit's reaction does not sweep.
        let mut mob = unit();
        assert_eq!(
            block_around(&mut mob, &around, MOB, &[(35, 2)]),
            [signal(MOB, 29), bridge(MOB, 29)]
        );
    }

    /// `0x600440`: the dead look signals the health and the power of the unit's power type, the
    /// tap `UNIT_FACTION`, any other bit nothing of its own; the bridge follows.
    #[test]
    fn the_dynamic_flags_watcher_signals_on_its_two_bits() {
        // Rage: `UNIT_FIELD_BYTES_0` byte 3 is 1.
        let mut mob = ObjectFields::from_pairs(&[(36, 1 << 24)]).into_created(ObjectType::Unit);
        assert_eq!(
            block(&mut mob, Some(ME), MOB, &[(143, 0x20)]),
            [signal(MOB, 16), signal(MOB, 18), bridge(MOB, 137)]
        );
        assert_eq!(
            block(&mut mob, Some(ME), MOB, &[(143, 0x24)]),
            [signal(MOB, 29), bridge(MOB, 137)]
        );
        assert_eq!(
            block(&mut mob, Some(ME), MOB, &[(143, 0x2c)]),
            [bridge(MOB, 137)]
        );
    }

    /// Every player's flags signal `PLAYER_FLAGS_CHANGED` on any bit, after the reaction 0x180
    /// runs; `PLAYER_BYTES_3` bytes 2 and 3 name the player, byte 3 refreshing its reaction too,
    /// ahead of the active player's own rank watch on the same dword.
    #[test]
    fn the_player_type_watchers_reach_every_player() {
        let mut other = player();
        assert_eq!(
            block(&mut other, Some(ME), OTHER, &[(190, 0x2)]),
            [signal(OTHER, 407)]
        );
        assert_eq!(
            block(&mut other, Some(ME), OTHER, &[(190, 0x102)]),
            [signal(OTHER, 29), signal(OTHER, 407)]
        );
        assert_eq!(
            block(&mut other, Some(ME), OTHER, &[(195, 0x0100_0000)]),
            [signal(OTHER, 183), signal(OTHER, 29)]
        );
        let mut me = player();
        assert_eq!(
            block(&mut me, Some(ME), ME, &[(195, 0x0001_0000)]),
            [signal(ME, 183), FieldNotice::Player { id: 524 }]
        );
        // The duel team.
        assert_eq!(
            block(&mut other, Some(ME), OTHER, &[(196, 1)]),
            [signal(OTHER, 29)]
        );
    }

    /// The quest-log watch is a player's, one for the whole log; a creature has none.
    #[test]
    fn the_quest_log_watch_is_one_per_block_on_a_player() {
        let mut other = player();
        assert_eq!(
            block(&mut other, Some(ME), OTHER, &[(198, 5), (257, 1)]),
            [bridge(OTHER, 522)]
        );
        // Field 258, past the 60 dwords, is outside it: the visible items' watch begins there.
        assert_eq!(
            block(&mut other, Some(ME), OTHER, &[(258, 1)]),
            [bridge(OTHER, id::UNIT_INVENTORY_CHANGED)]
        );
    }

    /// The active player's log carries the quest log's own watcher (`0x4de430`, registered after
    /// the bridge's on the same 0xf0 bytes): one run a block, however many slots moved, and none
    /// for an equal rewrite.
    #[test]
    fn the_active_players_quest_log_watcher_runs_once_per_block() {
        let mut me = player();
        assert_eq!(
            block(
                &mut me,
                Some(ME),
                ME,
                &[(198, 5), (199, 3), (201, 7), (257, 1)]
            ),
            [bridge(ME, 522), FieldNotice::QuestLog]
        );
        assert!(block(&mut me, Some(ME), ME, &[(198, 5), (201, 7)]).is_empty());
    }

    /// The pet number signals the classification; an entry change names the unit only when the
    /// creature cache holds the new entry, a miss waiting for the answer.
    #[test]
    fn the_pet_number_and_a_cached_entry_signal() {
        let mut mob = unit();
        assert_eq!(
            block(&mut mob, Some(ME), MOB, &[(139, 4)]),
            [signal(MOB, 187)]
        );
        let cached = Around {
            me: Some(ME),
            creature_lookup: &|entry| entry == 299,
            others: &Vec::new,
            pet_bar: 0,
        };
        assert_eq!(
            block_around(&mut mob, &cached, MOB, &[(3, 299)]),
            [signal(MOB, 183)]
        );
        assert!(block_around(&mut mob, &cached, MOB, &[(3, 300)]).is_empty());
    }

    /// The pet bar's unit adds `PET_UI_UPDATE` after its classification signal (`0x604c16`); any
    /// other unit's pet number signals the classification alone.
    #[test]
    fn the_bar_pets_number_signals_the_pet_ui() {
        let bar = Around {
            me: Some(ME),
            creature_lookup: &|_| false,
            others: &Vec::new,
            pet_bar: PET,
        };
        let mut pet = unit();
        assert_eq!(
            block_around(&mut pet, &bar, PET, &[(139, 4)]),
            [signal(PET, 187), FieldNotice::Global { id: 427 }]
        );
        let mut mob = unit();
        assert_eq!(
            block_around(&mut mob, &bar, MOB, &[(139, 4)]),
            [signal(MOB, 187)]
        );
    }

    /// `0x5ee990`'s arms below the active-player gate, each on its own bits, after
    /// `PLAYER_FLAGS_CHANGED` and in the callback's order; another player's flags reach none.
    #[test]
    fn the_active_players_flag_arms_signal_on_their_own_bits() {
        let flags = pf(PLAYER_FLAGS.0);
        let changed = signal(ME, 407);
        let resting = FieldNotice::Global { id: 405 };
        let rest_tutorial = FieldNotice::Tutorial {
            id: tutorial::RESTING,
        };
        let playtime = FieldNotice::Global { id: 530 };
        let mut me = player();
        // `PLAYER_FLAGS_HIDE_HELM`, a bit no arm tests.
        assert_eq!(block(&mut me, Some(ME), ME, &[(flags, 0x400)]), [changed]);
        // The resting bit, both edges (`0x5eead0`), its tutorial on the set one (`0x5eeae6`).
        assert_eq!(
            block(&mut me, Some(ME), ME, &[(flags, 0x420)]),
            [changed, rest_tutorial, resting]
        );
        assert_eq!(
            block(&mut me, Some(ME), ME, &[(flags, 0x400)]),
            [changed, resting]
        );
        // The two play-time bits are one arm (`0x5eeb65 test ah,0x30`).
        assert_eq!(
            block(&mut me, Some(ME), ME, &[(flags, 0x1400)]),
            [changed, playtime]
        );
        assert_eq!(
            block(&mut me, Some(ME), ME, &[(flags, 0x3400)]),
            [changed, playtime]
        );
        // The PvP preference names its new state.
        assert_eq!(
            block(&mut me, Some(ME), ME, &[(flags, 0x3600)]),
            [changed, FieldNotice::PvpPreference { on: true }]
        );
        // Every arm from one dword, in the callback's order.
        assert_eq!(
            block(&mut me, Some(ME), ME, &[(flags, 0x420)]),
            [
                changed,
                rest_tutorial,
                resting,
                FieldNotice::PvpPreference { on: false },
                playtime
            ]
        );
        let mut other = player();
        assert_eq!(
            block(&mut other, Some(ME), OTHER, &[(flags, 0x3220)]),
            [signal(OTHER, 407)]
        );
    }

    /// `0x5ff580` on `UNIT_FIELD_FLAGS`: the active player's combat bit first (`0x5ff4e0`), the
    /// reactions, then the combat bit of a unit whose charmer, else summoner, is the player.
    #[test]
    fn the_unit_flags_arms_signal_combat_for_the_player_and_its_pet() {
        let mut me = player();
        assert_eq!(
            block(&mut me, Some(ME), ME, &[(46, 0x8_0000)]),
            [FieldNotice::Global { id: 190 }, bridge(ME, 40)]
        );
        assert_eq!(
            block(&mut me, Some(ME), ME, &[(46, 0x8_0080)]),
            [signal(ME, 29), bridge(ME, 40)],
            "the combat bit did not move"
        );
        assert_eq!(
            block(&mut me, Some(ME), ME, &[(46, 0)]),
            [
                FieldNotice::Global { id: 191 },
                signal(ME, 29),
                bridge(ME, 40)
            ]
        );
        // Another unit's combat bit is no player's regen.
        let mut mob = unit();
        assert_eq!(
            block(&mut mob, Some(ME), MOB, &[(46, 0x8_0000)]),
            [bridge(MOB, 40)]
        );

        // Ours by SUMMONEDBY: start and stop, after the reaction and before `UNIT_AURA`.
        let summoned = |by: u64| [(12, by as u32), (13, (by >> 32) as u32)];
        let mut pet = unit();
        block(&mut pet, Some(ME), PET, &summoned(ME));
        assert_eq!(
            block(&mut pet, Some(ME), PET, &[(46, 0x0800_1800)]),
            [
                signal(PET, 29),
                FieldNotice::Global { id: 334 },
                signal(PET, 41),
                bridge(PET, 40)
            ]
        );
        assert_eq!(
            block(&mut pet, Some(ME), PET, &[(46, 0x0800_1000)]),
            [FieldNotice::Global { id: 335 }, bridge(PET, 40)]
        );
        // Another player's minion, and ours charmed away: CHARMEDBY comes first (`0x5ff772`).
        let mut theirs = unit();
        block(&mut theirs, Some(ME), MOB, &summoned(OTHER));
        assert_eq!(
            block(&mut theirs, Some(ME), MOB, &[(46, 0x800)]),
            [bridge(MOB, 40)]
        );
        block(
            &mut pet,
            Some(ME),
            PET,
            &[(10, OTHER as u32), (11, (OTHER >> 32) as u32)],
        );
        assert!(!block(&mut pet, Some(ME), PET, &[(46, 0x0800_1800)])
            .contains(&FieldNotice::Global { id: 334 }));
        // Charmed by us, summoned by no one.
        let mut charmed = unit();
        block(&mut charmed, Some(ME), MOB, &[(10, ME as u32)]);
        assert_eq!(
            block(&mut charmed, Some(ME), MOB, &[(46, 0x800)]),
            [FieldNotice::Global { id: 334 }, bridge(MOB, 40)]
        );
    }

    /// The flag tutorials are the active player's watchers' own calls: the ghost state's when the
    /// ghost bit is now set (`0x5eeabc` → `0x5de9c0`, number 0x18); the rest tutorial when the
    /// resting bit is now set (`0x5eeae6`-`0x5eeaed`, 0x1d), ahead of `PLAYER_UPDATE_RESTING`
    /// (`0x5eeaf7`); the PvP tutorial when the PvP bit is now set, after its reaction (`0x5ff756`,
    /// 0x1f); and the rest state's row for state 1 (`0x80af50`: 0x19) after its line (`0x5de542`).
    /// A cleared bit, another unit and rest state 2 raise none.
    #[test]
    fn the_flag_tutorials_ride_their_watchers_for_the_active_player() {
        let flags = pf(PLAYER_FLAGS.0);
        let changed = signal(ME, 407);
        let resting = FieldNotice::Global { id: 405 };
        let tutorial = |id| FieldNotice::Tutorial { id };
        let mut me = player();
        assert_eq!(
            block(&mut me, Some(ME), ME, &[(flags, 0x20)]),
            [changed, tutorial(0x1d), resting]
        );
        assert_eq!(
            block(&mut me, Some(ME), ME, &[(flags, 0)]),
            [changed, resting]
        );
        assert_eq!(
            block(&mut me, Some(ME), ME, &[(flags, 0x10)]),
            [changed, tutorial(0x18)]
        );
        assert_eq!(block(&mut me, Some(ME), ME, &[(flags, 0)]), [changed]);
        let mut other = player();
        assert_eq!(
            block(&mut other, Some(ME), OTHER, &[(flags, 0x30)]),
            [signal(OTHER, 407)]
        );

        assert_eq!(
            block(&mut me, Some(ME), ME, &[(46, 0x1000)]),
            [signal(ME, 29), tutorial(0x1f), bridge(ME, 40)]
        );
        assert_eq!(
            block(&mut me, Some(ME), ME, &[(46, 0)]),
            [signal(ME, 29), bridge(ME, 40)]
        );
        let mut mob = unit();
        assert_eq!(
            block(&mut mob, Some(ME), MOB, &[(46, 0x1000)]),
            [signal(MOB, 29), bridge(MOB, 40)]
        );

        let rest = pf(REST_STATE);
        assert_eq!(
            block(&mut me, Some(ME), ME, &[(rest, 1 << 24)]),
            [FieldNotice::RestState { old: 0, new: 1 }, tutorial(0x19)]
        );
        assert_eq!(
            block(&mut me, Some(ME), ME, &[(rest, 2 << 24)]),
            [FieldNotice::RestState { old: 1, new: 2 }]
        );
    }

    /// `PLAYER_GUILDID` runs the guild tag on any player (`0x5e2770`), `PLAYER_GUILDRANK` signals
    /// `PLAYER_GUILD_UPDATE` bare (`0x5e27b0`).
    #[test]
    fn the_guild_watchers_reach_every_player() {
        let mut other = player();
        assert_eq!(
            block(&mut other, Some(ME), OTHER, &[(pf(GUILD_ID.0), 5)]),
            [FieldNotice::Global { id: 376 }, signal(OTHER, 345)]
        );
        assert_eq!(
            block(&mut other, Some(ME), OTHER, &[(pf(GUILD_RANK.0), 2)]),
            [FieldNotice::Global { id: 345 }]
        );
        let mut me = player();
        assert_eq!(
            block(
                &mut me,
                Some(ME),
                ME,
                &[(pf(GUILD_ID.0), 5), (pf(GUILD_RANK.0), 1)]
            ),
            [
                FieldNotice::Global { id: 376 },
                signal(ME, 345),
                FieldNotice::Global { id: 345 }
            ]
        );
        // A creature carries no player watchers.
        let mut mob = unit();
        assert!(block(&mut mob, Some(ME), MOB, &[(pf(GUILD_ID.0), 5)]).is_empty());
    }

    /// The active player's own charmed-by watch refreshes after the type watcher on the same
    /// field: two sweeps.
    #[test]
    fn the_active_players_charmed_by_watch_follows_the_type_watcher() {
        let mut me = player();
        let around = Around {
            me: Some(ME),
            creature_lookup: &|_| false,
            others: &|| vec![MOB],
            pet_bar: 0,
        };
        assert_eq!(
            block_around(&mut me, &around, ME, &[(10, 0x55)]),
            [
                signal(ME, 29),
                signal(MOB, 29),
                signal(ME, 29),
                signal(MOB, 29)
            ]
        );
        // `SUMMONEDBY` reaches the type watcher alone.
        assert_eq!(
            block_around(&mut me, &around, ME, &[(12, 0x55)]),
            [signal(ME, 29), signal(MOB, 29)]
        );
    }

    /// The sweep leaves out the player and what it charms or summoned, by charmer first.
    #[test]
    fn the_sweep_leaves_out_the_player_and_its_own() {
        let owned_by = |field: u16, owner: u64| {
            ObjectFields::from_pairs(&[(field, owner as u32), (field + 1, (owner >> 32) as u32)])
        };
        let mine = owned_by(12, ME);
        let charmed_away = {
            // Summoned by us but charmed by another: the charmer decides.
            let mut f = owned_by(12, ME);
            f.merge(owned_by(10, OTHER));
            f
        };
        let theirs = owned_by(12, OTHER);
        let me = ObjectFields::from_pairs(&[]);
        assert_eq!(
            reaction_sweep(
                ME,
                [
                    (ME, &me),
                    (PET, &mine),
                    (MOB, &charmed_away),
                    (OTHER, &theirs)
                ]
            ),
            [MOB, OTHER]
        );
    }
}
