//! The tutorial system (the reference's `[0x4b5150, 0x4b5a60)`) behind the stock
//! `TutorialFrame.xml`: two bit banks, the fire-once trigger, the acknowledge-and-send setter,
//! the timers, the popup sound and the trigger sites.
//!
//! Both banks (`0xb711b8`, `0xb711e4`) are filled from the same `SMSG_TUTORIAL_FLAGS`; until it
//! lands nothing fires. Bank A is fire-once: [`TutorialState::trigger`] tests and sets it and raises
//! `TUTORIAL_TRIGGER(id + 1)`, never sending. Bank B is acknowledged: [`TutorialState::acknowledge`]
//! tests it, sets both banks, cancels the id's timer and sends `CMSG_TUTORIAL_FLAG`. So A ⊇ B, and
//! doing the thing suppresses its tutorial, account-wide. `TutorialsEnabled` reads bank B through
//! the host at its call ([`TutorialLive`]).
//!
//! `TriggerTutorial(id, delayMs)` (`0x4b5390`): with a zero delay the `TutorialPopup` cue plays and
//! the event fires now; otherwise a timer holds it (the 10 s Targeting popup is the one site). The
//! reference bounds-checks nothing; an id past the bank is a no-op here.
//!
//! The reference has fifty-one trigger sites and seven acknowledge sites (the callers of
//! `0x4b5390` and `0x4b54c0`), each inside the handler, the input or the call that raises it. An
//! engine site with no VM in hand asks on the tutorials lane ([`Tutorials::trigger_now`]), stamped
//! where it runs, so the bank's test and its `TUTORIAL_TRIGGER` come at that place among the
//! packets' signals ([`crate::lanes`]). A site inside a lane's firing tests and fires in place
//! ([`TutorialState::fire_now`], [`fire_now`]); a call acknowledges in the call
//! ([`Tutorials::acknowledge_now`]); the sites beside a script queue on the VM.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use bevy::prelude::*;

use benilla_ui::script::{ScriptValue, TutorialCall, UiScript};

use crate::client_state::ClientState;
use crate::lanes::{LaneApp, Stamp, Until};
use crate::net::{ClientCommand, EnteredWorldMessage, NetCommands, ObjectStore, SelfPlayer};
use crate::player::Player;

/// The popup cue `0x4b5390` plays on both legs (`0x846b68`).
const TUTORIAL_POPUP_SOUND: &str = "TutorialPopup";

/// The internal (0-based) ids of the sites this app produces, named by their published title.
pub(crate) mod id {
    pub(crate) const QUESTGIVERS: u32 = 0x00;
    pub(crate) const MOVEMENT: u32 = 0x01;
    pub(crate) const CAMERAS: u32 = 0x02;
    pub(crate) const TARGETING: u32 = 0x03;
    pub(crate) const COMBAT_MODE: u32 = 0x04;
    pub(crate) const SPELLS_AND_ABILITIES: u32 = 0x05;
    pub(crate) const LOOTING: u32 = 0x06;
    pub(crate) const BACKPACK: u32 = 0x07;
    pub(crate) const FOOD: u32 = 0x0a;
    pub(crate) const DRINK: u32 = 0x0b;
    pub(crate) const USING_ITEMS: u32 = 0x08;
    pub(crate) const BAGS: u32 = 0x09;
    pub(crate) const LEARNING_TALENTS: u32 = 0x0c;
    pub(crate) const TRAINERS: u32 = 0x0d;
    pub(crate) const REPUTATION: u32 = 0x0f;
    pub(crate) const REPLYING_TO_TELLS: u32 = 0x10;
    pub(crate) const GROUPING: u32 = 0x11;
    pub(crate) const PLAYERS: u32 = 0x12;
    pub(crate) const VENDORS: u32 = 0x13;
    pub(crate) const QUEST_LOG: u32 = 0x14;
    pub(crate) const FRIENDS: u32 = 0x15;
    pub(crate) const CHATTING: u32 = 0x16;
    pub(crate) const EQUIPPABLE_ITEMS: u32 = 0x17;
    pub(crate) const DEATH: u32 = 0x18;
    pub(crate) const RESTED: u32 = 0x19;
    pub(crate) const FATIGUE: u32 = 0x1a;
    pub(crate) const SWIMMING: u32 = 0x1b;
    pub(crate) const BREATH: u32 = 0x1c;
    pub(crate) const RESTING: u32 = 0x1d;
    pub(crate) const HEARTHSTONES: u32 = 0x1e;
    pub(crate) const PVP_COMBAT: u32 = 0x1f;
    pub(crate) const TRAVEL: u32 = 0x22;
    pub(crate) const SPELLBOOK: u32 = 0x27;
    pub(crate) const ELITE_QUESTS: u32 = 0x28;
    pub(crate) const WELCOME: u32 = 0x29;
    pub(crate) const UNAVAILABLE_QUEST_GIVERS: u32 = 0x2a;
    pub(crate) const RANGED_WEAPONS: u32 = 0x2b;
    pub(crate) const AMMUNITION: u32 = 0x2c;
    pub(crate) const RAID_GROUPS: u32 = 0x2d;
    pub(crate) const MEETING_STONES: u32 = 0x2e;
    pub(crate) const BATTLEGROUND_QUEUE: u32 = 0x2f;
    pub(crate) const PORT_TO_BATTLEGROUND: u32 = 0x30;
    pub(crate) const KEYRINGS: u32 = 0x31;
}

/// The level-gated arm of the `SMSG_LEVELUP_INFO` handler (`0x5e38c0`, `0x5e4086`-`0x5e410e`): in
/// the handler's own order, each fires when the message's level is at least its threshold
/// (`crate::ui_unit::field_events`).
pub(crate) const LEVEL_TRIGGERS: [(i32, u32); 8] = [
    (3, 0x16),  // Chatting
    (4, 0x0d),  // Trainers
    (4, 0x02),  // Cameras
    (5, 0x26),  // Groups
    (7, 0x15),  // Friends
    (7, 0x25),  // Professions
    (8, 0x20),  // Jumping
    (10, 0x0c), // Learning Talents
];

/// What StopAttack (`0x5ecac0`) reads of the active player for its two tutorials, the stop's
/// own site (`0x5ecaf7`-`0x5ecb54`): Food (`0x0a`) below half health, Drink (`0x0b`) for a mana
/// user (`UNIT_FIELD_BYTES_0` byte 3 zero, `0x5ecb22`) below half mana, or with no maximum at all
/// (`0x5ecb2c`-`0x5ecb37`). Each ratio is signed (`fild`) and strictly below `0.5` (`0x7ffa24`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct LowVitals {
    food: bool,
    drink: bool,
}

impl LowVitals {
    /// The two tests off the active player's descriptor; nothing without one.
    pub(crate) fn of(own: Option<&ObjectStore>) -> Self {
        let Some(f) = own.map(|s| &s.0) else {
            return Self::default();
        };
        // A 0 maximum divides to an infinity or NaN, neither below half (`0x5ecb0e`).
        let below_half = |cur: Option<u32>, max: Option<u32>| {
            let (cur, max) = (cur.unwrap_or(0) as i32, max.unwrap_or(0) as i32);
            f64::from(cur) / f64::from(max) < 0.5
        };
        let max_mana = f.unit_max_power(0).unwrap_or(0);
        Self {
            food: below_half(f.unit_health(), f.unit_max_health()),
            drink: f.unit_power_type() == 0
                && (max_mana == 0 || below_half(f.unit_power(0), Some(max_mana))),
        }
    }

    /// The tutorials owed, in the stop's order.
    pub(crate) fn tutorials(self) -> impl Iterator<Item = u32> {
        [(self.food, id::FOOD), (self.drink, id::DRINK)]
            .into_iter()
            .filter_map(|(owed, id)| owed.then_some(id))
    }
}

/// What `SetSelection`'s tutorial ladder reads of the unit it lit, against the active player.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SelectFacts {
    /// Type bit 4 (`0x493792`).
    pub(crate) player: bool,
    /// The active player's own guid (`0x4937a1`-`0x4937a7`).
    pub(crate) me: bool,
    /// `CanCooperate(player, unit)` (`0x4937e0`).
    pub(crate) cooperates: bool,
    /// The active player's `UNIT_FIELD_LEVEL` (`0x4937f2`).
    pub(crate) my_level: i32,
    /// `CanAttack(player, unit)` (`0x49380a`).
    pub(crate) attackable: bool,
    /// `UNIT_NPC_FLAGS` bit 3, flight master (`0x49382d`).
    pub(crate) flight_master: bool,
}

/// `SetSelection`'s ladder, first match: Players for another player while its fire-once bit is
/// clear (`0x4937af`, else it falls through), Grouping for a cooperative player at the active
/// player's level 5 or more, Combat Mode for an attackable unit, Travel for a flight master.
pub(crate) fn select_tutorial(f: SelectFacts, players_fired: bool) -> Option<u32> {
    if f.player && !f.me && !players_fired {
        Some(id::PLAYERS)
    } else if f.player && f.cooperates && f.my_level >= 5 {
        Some(id::GROUPING)
    } else if f.attackable {
        Some(id::COMBAT_MODE)
    } else if f.flight_master {
        Some(id::TRAVEL)
    } else {
        None
    }
}

/// The Targeting popup's delay at its one site (`0x514a8d mov edx,0x2710`).
pub(crate) const TARGETING_DELAY_MS: u32 = 10_000;

/// The Movement popup's silence window (`0x482ff7`: elapsed - stamp - 90000 >= 0), taken as 90 s
/// after world enter with no movement input; what the stamp (`0x5143e0`) holds is untraced.
const MOVEMENT_SILENCE: Duration = Duration::from_millis(90_000);

/// A site's ask of the tutorial system, stamped on the tutorials lane where the site runs and
/// applied at that stamp ([`fire_tutorials`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TutorialAsk {
    /// `TriggerTutorial(id, delayMs)` (`0x4b5390`): the fire-once test, then the event.
    Trigger { id: u32, delay_ms: u32 },
    /// `SetTutorialFlag(id)` (`0x4b54c0`): the acknowledge, and its `CMSG_TUTORIAL_FLAG`.
    Acknowledge { id: u32 },
    /// A published id whose bit its site already set: a due timer's (`0x4b5840`), or a site's
    /// that fired with no VM to hear it.
    Fire { published: u32 },
    /// `SetSelection` (`0x493540`) lit `entity`, `guid`'s object: its ladder
    /// (`0x493789`-`0x49383c`, [`select_tutorial`]).
    Selected { entity: Entity, guid: u64 },
    /// `SMSG_TUTORIAL_FLAGS` (`0x4b5700`): both banks copied from the payload.
    Flags(Vec<u8>),
}

/// A send the acknowledges and the verbs owe the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TutorialSend {
    Flag(u32),
    Clear,
    Reset,
}

/// A bank bit, byte-wise: the client's `id >> 5` dword indexing on little-endian dwords.
fn bank_bit(bank: &[u8], id: u32) -> Option<bool> {
    bank.get((id >> 3) as usize)
        .map(|b| b & (1 << (id & 7)) != 0)
}

fn set_bank_bit(bank: &mut [u8], id: u32) {
    if let Some(b) = bank.get_mut((id >> 3) as usize) {
        *b |= 1 << (id & 7);
    }
}

/// The tutorial state, shared through `&self` ([`Self::lock`]): the call host acknowledges inside
/// a script call (`InviteToParty`'s `SetTutorialFlag` at `0x5e949c`) while systems that lend the
/// host hold it too.
#[derive(Resource, Default)]
pub(crate) struct Tutorials(std::sync::Mutex<TutorialState>);

impl Tutorials {
    /// The state, held for one step; never across Lua, which may acknowledge in a call.
    pub(crate) fn lock(&self) -> std::sync::MutexGuard<'_, TutorialState> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// `SetTutorialFlag(id)` (`0x4b54c0`) inside a script call or an input: the acknowledge, and
    /// its `CMSG_TUTORIAL_FLAG` sent at once, with any send owed before it, so it leaves ahead of
    /// the call's own packet.
    pub(crate) fn acknowledge_now(&self, id: u32, commands: &NetCommands) {
        let mut state = self.lock();
        state.acknowledge(id);
        state.flush(commands);
    }

    /// `TriggerTutorial(id, 0)` (`0x4b5390`) from an engine site with no VM in hand: stamped here
    /// on the tutorials lane, where the fire-once test and its `TUTORIAL_TRIGGER` run, so a
    /// handler's fires after every earlier packet's signals and ahead of every later one's.
    pub(crate) fn trigger_now(&self, id: u32) {
        self.lock().push(TutorialAsk::Trigger { id, delay_ms: 0 });
    }

    /// `SetTutorialFlag(id)` (`0x4b54c0`) from a handler: stamped here on the tutorials lane, where
    /// the banks take it and its packet goes out.
    pub(crate) fn acknowledge_at_place(&self, id: u32) {
        self.lock().push(TutorialAsk::Acknowledge { id });
    }

    /// `SetSelection` (`0x493540`) lit `entity`, `guid`'s object: its tutorial ladder
    /// (`0x493789`-`0x49383c`) is judged at this place on the tutorials lane ([`select_tutorial`]).
    pub(crate) fn selected(&self, entity: Entity, guid: u64) {
        self.lock().push(TutorialAsk::Selected { entity, guid });
    }

    /// `TutorialsEnabled()` (`0x4b5960`): any byte of bank B's whole `[0xb711ec] * 4` bytes, not
    /// just the fifty ids, is not 0xFF; false with no bank.
    pub(crate) fn enabled(&self) -> bool {
        self.lock()
            .acknowledged
            .as_ref()
            .is_some_and(|b| b.iter().any(|&byte| byte != 0xFF))
    }

    /// The asks the lane holds, in order, taken unapplied.
    #[cfg(test)]
    pub(crate) fn take_asks(&self) -> Vec<TutorialAsk> {
        std::mem::take(&mut self.lock().lane)
            .into_iter()
            .map(|(_, ask)| ask)
            .collect()
    }

    /// The published ids the lane's asks fire, each applied in its order but for the selections,
    /// which stay for the lane's firing.
    #[cfg(test)]
    pub(crate) fn take_fired(&self) -> Vec<u32> {
        let mut state = self.lock();
        let asks = std::mem::take(&mut state.lane);
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let mut fired = Vec::new();
        for (stamp, ask) in asks {
            if matches!(ask, TutorialAsk::Selected { .. }) {
                state.lane.push_back((stamp, ask));
            } else if let Some(published) = state.apply(ask, Instant::now(), &commands) {
                fired.push(published);
            }
        }
        fired
    }
}

/// The two banks, the timers, the lane and what the acknowledges owe the wire.
#[derive(Default)]
pub(crate) struct TutorialState {
    /// Bank A (`0xb711b8`…): fire-once. `None` until the packet lands.
    fire_once: Option<Vec<u8>>,
    /// Bank B (`0xb711e4`…): acknowledged.
    acknowledged: Option<Vec<u8>>,
    /// The delayed triggers (`TUTORIALTIMER` nodes): `(id, due)`.
    timers: Vec<(u32, Instant)>,
    sends: Vec<TutorialSend>,
    /// The tutorials lane: each site's ask, stamped where it ran, in stamp order.
    lane: VecDeque<(Stamp, TutorialAsk)>,
}

impl TutorialState {
    /// Queue `ask` on the lane, stamped now, behind everything stamped before it.
    fn push(&mut self, ask: TutorialAsk) {
        // A trigger or a selection's ladder a call queued fires at its host's next point
        // ([`TutorialSteps::fire_since`]): the census counts a send made before then.
        #[cfg(test)]
        if matches!(
            ask,
            TutorialAsk::Trigger { .. } | TutorialAsk::Fire { .. } | TutorialAsk::Selected { .. }
        ) {
            benilla_ui::script::census::queued();
        }
        let stamp = crate::lanes::stamp();
        let at = self.lane.partition_point(|(s, _)| *s < stamp);
        self.lane.insert(at, (stamp, ask));
    }

    fn head(&self) -> Option<Stamp> {
        self.lane.front().map(|(s, _)| *s)
    }

    /// The asks stamped since `mark`, taken in order: a call's own, for its host to apply at its
    /// point.
    fn take_since(&mut self, mark: crate::lanes::Mark) -> Vec<TutorialAsk> {
        let Some(from) = self.lane.iter().position(|(s, _)| s.since(mark)) else {
            return Vec::new();
        };
        self.lane.drain(from..).map(|(_, ask)| ask).collect()
    }

    /// The head, taken if `until` admits it.
    fn pop(&mut self, until: Until) -> Option<(Stamp, TutorialAsk)> {
        until
            .admits(self.head()?)
            .then(|| self.lane.pop_front())
            .flatten()
    }

    /// One ask applied at its stamp: the published id it fires, if any. A selection's ladder needs
    /// the world's reads and is judged by the lane's firing.
    fn apply(&mut self, ask: TutorialAsk, now: Instant, commands: &NetCommands) -> Option<u32> {
        match ask {
            TutorialAsk::Trigger { id, delay_ms } => self.trigger(id, delay_ms, now),
            TutorialAsk::Acknowledge { id } => {
                self.acknowledge(id);
                self.flush(commands);
                None
            }
            TutorialAsk::Fire { published } => Some(published),
            TutorialAsk::Flags(bytes) => {
                self.apply_flags(&bytes);
                None
            }
            TutorialAsk::Selected { .. } => None,
        }
    }

    /// Send what is owed, in order.
    fn flush(&mut self, commands: &NetCommands) {
        for send in std::mem::take(&mut self.sends) {
            let command = match send {
                TutorialSend::Flag(id) => ClientCommand::TutorialFlag { id },
                TutorialSend::Clear => ClientCommand::TutorialClear,
                TutorialSend::Reset => ClientCommand::TutorialReset,
            };
            let _ = commands.0.send(command);
        }
    }

    /// `SMSG_TUTORIAL_FLAGS`: both banks copied from the payload.
    pub(crate) fn apply_flags(&mut self, bytes: &[u8]) {
        self.fire_once = Some(bytes.to_vec());
        self.acknowledged = Some(bytes.to_vec());
    }

    /// The bring-up `0x4b5330`: both banks zeroed if present (the bit count stays).
    fn world_enter(&mut self) {
        for bank in [&mut self.fire_once, &mut self.acknowledged]
            .into_iter()
            .flatten()
        {
            bank.iter_mut().for_each(|b| *b = 0);
        }
    }

    /// The teardown `0x4b5380`: every pending timer flushed, and what the lane held for the
    /// interface that was up.
    fn world_leave(&mut self) {
        self.timers.clear();
        self.lane.clear();
    }

    /// `TriggerTutorial(id, delayMs)` (`0x4b5390`): the published id to fire now for no delay;
    /// else the timer armed and nothing yet.
    pub(crate) fn trigger(&mut self, id: u32, delay_ms: u32, now: Instant) -> Option<u32> {
        let published = self.fire_now(id)?;
        if delay_ms == 0 {
            return Some(published);
        }
        self.timers
            .push((id, now + Duration::from_millis(u64::from(delay_ms))));
        None
    }

    /// `0x4b5390`'s fire-once test: bank A's bit for `id` tested and set, and the published id
    /// (`id + 1`) for the caller to fire, at once with no delay ([`fire_published`]). `None` with
    /// no bank yet, the bit already set, or an id past the bank (the reference would overrun).
    pub(crate) fn fire_now(&mut self, id: u32) -> Option<u32> {
        let bank = self.fire_once.as_mut()?;
        (bank_bit(bank, id) == Some(false)).then(|| {
            set_bank_bit(bank, id);
            id + 1
        })
    }

    /// Bank A's bit for `id`, as `SetSelection` tests Players' directly (`0x4937a9`-`0x4937af`);
    /// clear with no bank.
    fn has_fired(&self, id: u32) -> bool {
        self.fire_once
            .as_deref()
            .and_then(|bank| bank_bit(bank, id))
            .unwrap_or(false)
    }

    /// `SetTutorialFlag(id)` (`0x4b54c0`): bank B's bit gates, both banks set, the id's timer
    /// cancelled, the 0-based id sent. The reference dereferences an absent bank B; here it is a
    /// no-op.
    pub(crate) fn acknowledge(&mut self, id: u32) {
        let Some(bank_b) = self.acknowledged.as_mut() else {
            return;
        };
        if bank_bit(bank_b, id) != Some(false) {
            return;
        }
        self.timers.retain(|(t, _)| *t != id);
        set_bank_bit(bank_b, id);
        if let Some(bank_a) = self.fire_once.as_mut() {
            set_bank_bit(bank_a, id);
        }
        self.sends.push(TutorialSend::Flag(id));
    }

    /// `ClearTutorials()`: the timers flushed, every bit of both banks set, the empty clear sent.
    fn clear(&mut self) {
        self.timers.clear();
        for bank in [&mut self.fire_once, &mut self.acknowledged]
            .into_iter()
            .flatten()
        {
            bank.iter_mut().for_each(|b| *b = 0xFF);
        }
        self.sends.push(TutorialSend::Clear);
    }

    /// `ResetTutorials()`: every bit of both banks cleared, the timers kept, the empty reset sent.
    fn reset(&mut self) {
        for bank in [&mut self.fire_once, &mut self.acknowledged]
            .into_iter()
            .flatten()
        {
            bank.iter_mut().for_each(|b| *b = 0);
        }
        self.sends.push(TutorialSend::Reset);
    }

    /// `ProcessTutorialTimers`: the published ids of every due node, which fire the same cue and
    /// event untested.
    fn tick(&mut self, now: Instant) -> Vec<u32> {
        let mut due = Vec::new();
        self.timers.retain(|&(id, at)| {
            if at <= now {
                due.push(id + 1);
                false
            } else {
                true
            }
        });
        due
    }

    #[cfg(test)]
    fn bits(&self) -> (Option<&[u8]>, Option<&[u8]>) {
        (self.fire_once.as_deref(), self.acknowledged.as_deref())
    }
}

/// The last-seen swimming state, for the edge-triggered `0x4b5390` site polled here. The ghost,
/// resting, PvP and rested tutorials ride their field watchers ([`crate::field_watch`]), the
/// ghost's arrival leg the arrival's ghost state ([`fire_now`]), the level tutorials the
/// level-up message (`crate::ui_unit::field_events`).
#[derive(Default)]
struct SelfWatch {
    swimming: Option<bool>,
}

/// The hearthstone's item entry (`0x1b24`).
const HEARTHSTONE: u32 = 6948;
const INVTYPE_RANGED: u32 = 15;
/// The equipment-slot mask per inventory type 0 to 28 (`0x809200`): bit `n` is slot `n`. Types 0
/// (not equippable), 24 (ammo) and 27 (quiver) fit no slot.
const INVENTORY_SLOT_MASK: [u32; 29] = [
    0, 0x1, 0x2, 0x4, 0x8, 0x10, 0x20, 0x40, 0x80, 0x100, 0x200, 0xc00, 0x3000, 0x18000, 0x10000,
    0x20000, 0x4000, 0x8000, 0x780000, 0x40000, 0x10, 0x8000, 0x10000, 0x10000, 0, 0x20000,
    0x20000, 0, 0x20000,
];
const CLASS_HUNTER: u8 = 3;

/// The item-received function's tutorials for our own push, in its order (`0x491cec`-`0x491ec9`),
/// which runs with the item's record held, at the push's packet or at the record's answer
/// (`0x491ee0`): Backpack always; Using Items for an on-use spell; Equippable Items for an inventory
/// type that fits a slot ([`INVENTORY_SLOT_MASK`]); Bags for a container; Keyrings for the keyring
/// band; Hearthstones by entry; Ranged Weapons for a ranged type on a non-hunter.
pub(crate) fn item_tutorials(
    entry: u32,
    keyring: bool,
    info: &benilla_protocol::messages::ItemInfo,
    class: Option<u8>,
) -> Vec<u32> {
    // A spell in any of the five blocks with trigger 0, ON_USE (`0x491d00`-`0x491d0a`).
    let on_use = info.spells.iter().any(|s| s.trigger == 0);
    // A set bit among the first 23 of the type's slot mask (`0x491d23`-`0x491d3f`).
    let equippable = INVENTORY_SLOT_MASK
        .get(info.inventory_type as usize)
        .is_some_and(|m| m & 0x7f_ffff != 0);
    [
        (true, id::BACKPACK),
        (on_use, id::USING_ITEMS),
        (equippable, id::EQUIPPABLE_ITEMS),
        // Class 1, Container (`0x491d4f cmp [esi],1`).
        (info.class == 1, id::BAGS),
        // Bag 255, slots 0x51 to 0x70 (`0x491d60`-`0x491d73`).
        (keyring, id::KEYRINGS),
        (entry == HEARTHSTONE, id::HEARTHSTONES),
        (
            info.inventory_type == INVTYPE_RANGED && class != Some(CLASS_HUNTER),
            id::RANGED_WEAPONS,
        ),
    ]
    .into_iter()
    .filter_map(|(owed, id)| owed.then_some(id))
    .collect()
}

/// The tutorials lane's head ([`crate::lanes`]).
fn peek_tutorials(world: &World) -> Option<Stamp> {
    world.get_resource::<Tutorials>()?.lock().head()
}

/// The lane's firing: each ask the drain admits, applied at its stamp, its `TUTORIAL_TRIGGER`
/// fired with the host lent. The bank is free while Lua runs: a listener may acknowledge in a
/// call. It stops after an ask whose firing stamped anything, so what that pushed fires next.
fn fire_tutorials(
    In(until): In<Until>,
    mut vm: crate::script_calls::VmWith<SelectReads<'static, 'static>>,
    tutorials: Res<Tutorials>,
    sounds: Res<crate::sound::MessageSounds>,
    commands: Res<NetCommands>,
) {
    if !vm.run(|script| script.is_some()) {
        return;
    }
    loop {
        let next = tutorials.lock().pop(until);
        let Some((at, ask)) = next else {
            return;
        };
        let now = Instant::now();
        let published = match ask {
            TutorialAsk::Selected { entity, guid } => {
                let facts = vm.own().facts(entity, guid);
                let mut state = tutorials.lock();
                facts
                    .and_then(|f| select_tutorial(f, state.has_fired(id::PLAYERS)))
                    .and_then(|id| state.trigger(id, 0, now))
            }
            ask => tutorials.lock().apply(ask, now, &commands),
        };
        let Some(published) = published else {
            continue;
        };
        let ((), moved) = crate::lanes::firing(at, || {
            vm.run(|script| {
                if let Some(script) = script {
                    fire_published(script, &sounds, published);
                }
            })
        });
        if moved {
            return;
        }
    }
}

/// What the selection ladder reads of the lit unit and the active player.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct SelectReads<'w, 's> {
    self_q: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
    stores: Query<'w, 's, &'static ObjectStore>,
    relations: crate::target::RelationWorld<'w, 's>,
}

impl SelectReads<'_, '_> {
    /// What the ladder reads of the lit unit `guid`, `entity`'s object, against the active
    /// player; `None` without either, or for an object that is no unit (`0x49370f`).
    fn facts(&self, entity: Entity, guid: u64) -> Option<SelectFacts> {
        const TYPE_UNIT: u32 = 0x8;
        const TYPE_PLAYER: u32 = 0x10;
        const NPC_FLIGHT_MASTER: u32 = 0x8;
        let store = self.stores.get(entity).ok()?;
        let mask = store.0.object_type_mask().unwrap_or(0);
        if mask & TYPE_UNIT == 0 {
            return None;
        }
        let relations = &self.relations;
        let me = relations.me()?;
        let unit = relations.unit_of(guid, store);
        let rel = relations.get();
        let own = self.self_q.single().ok();
        Some(SelectFacts {
            player: mask & TYPE_PLAYER != 0,
            me: relations.me_guid() == Some(guid),
            cooperates: rel.can_cooperate(&me, &unit),
            my_level: own.and_then(|s| s.0.unit_level()).unwrap_or(0) as i32,
            attackable: rel.can_attack(&me, &unit),
            flight_master: store.0.unit_npc_flags() & NPC_FLIGHT_MASTER != 0,
        })
    }
}

/// `TutorialsEnabled` (`0x4b5960`) at its call: bank B as it stands, through the host.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct TutorialLive<'w> {
    /// Absent in a harness without the tutorials, whose VM reads its own copy.
    tutorials: Option<Res<'w, Tutorials>>,
}

impl crate::script_calls::live::LiveFamily for TutorialLive<'_> {
    fn answer(&mut self, ask: &mut benilla_ui::script::live::LiveAsk<'_>) -> bool {
        ask.answer(|_: &benilla_ui::script::TutorialsEnabledQuery| {
            Some(self.tutorials.as_deref()?.enabled())
        })
    }
}

/// The game timer's callback `0x4b5840`, at the frame's head: each due timer's published id,
/// stamped ahead of the frame's input and packets, as the timers run first in the tick
/// (`0x420d0c`).
fn tick_tutorial_timers(tutorials: Res<Tutorials>) {
    let mut state = tutorials.lock();
    for published in state.tick(Instant::now()) {
        state.push(TutorialAsk::Fire { published });
    }
}

/// Before the drain: the triggers the VM's sites beside a script queued, stamped here: Quest
/// Completion (`0x4df792`), Replying to Tells (`0x49b107`), the Quest Log (`0x5013df`), and the
/// inventory alerts' (`0x4c8002`, `0x4c802e`) from a pass that ran with no host open; with one,
/// those fire in the pass ([`TutorialCall::Trigger`]).
fn stamp_script_triggers(mut hosted_vm: crate::script_calls::HostedVm, tutorials: Res<Tutorials>) {
    let ids = hosted_vm.run(|script| script.map(UiScript::take_tutorial_triggers));
    let mut state = tutorials.lock();
    for id in ids.into_iter().flatten() {
        state.push(TutorialAsk::Trigger { id, delay_ms: 0 });
    }
}

/// `0x4b5390`'s firing of a published id: the `TutorialPopup` cue, then `TUTORIAL_TRIGGER`.
pub(crate) fn fire_published(
    script: &mut UiScript,
    sounds: &crate::sound::MessageSounds,
    published: u32,
) {
    sounds.push_cue(TUTORIAL_POPUP_SOUND);
    script.fire_event(
        "TUTORIAL_TRIGGER",
        vec![ScriptValue::Int(i64::from(published))],
    );
}

/// The polled `0x4b5390` edge site, `StartSwimming`'s (`0x60df70`, `0x60dfa1`): the mover entering
/// water, at the frame time that moves it. It fires on a change, never at the login; the bring-up
/// resets the memory.
fn watch_self(
    player: Res<Player>,
    mut cascade: ResMut<WorldEnterCascade>,
    mut watch: Local<SelfWatch>,
    tutorials: Res<Tutorials>,
) {
    if std::mem::take(&mut cascade.reset_watch) {
        *watch = SelfWatch::default();
    }
    if let Some(prev) = watch.swimming {
        if !prev && player.swimming {
            tutorials.trigger_now(id::SWIMMING);
        }
    }
    watch.swimming = Some(player.swimming);
}

/// The tutorial verbs' engine half ([`crate::script_calls::ScriptCallHost`]): each writes the banks
/// and sends before the verb returns, and `TutorialsEnabled` (`0x4b5960`) reads the write on the
/// verb's next line ([`TutorialLive`]).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct TutorialSteps<'w, 's> {
    /// Absent in a harness with no tutorial state, which drops the calls.
    tutorials: Option<Res<'w, Tutorials>>,
    commands: Res<'w, NetCommands>,
    /// Absent in a harness without the sounds, whose triggers fire with no cue.
    sounds: Option<Res<'w, crate::sound::MessageSounds>>,
    /// What a selection's ladder reads where its call applies it.
    reads: SelectReads<'w, 's>,
}

impl TutorialSteps<'_, '_> {
    /// The tutorial asks a call's engine half queued since `mark` ([`Tutorials::trigger_now`]),
    /// applied at its signal point: a `TriggerTutorial(id, 0)` (`0x4b5390`) runs its fire-once
    /// test, then the cue and `TUTORIAL_TRIGGER` where the call reached, as the reference fires
    /// it inside its verb; a selection's ladder (`0x493789`-`0x49383c`) is judged there too, ahead
    /// of its send (`0x49388d`).
    pub(crate) fn fire_since(&mut self, script: &mut UiScript, mark: crate::lanes::Mark) {
        let Some(tutorials) = self.tutorials.as_deref() else {
            return;
        };
        let asks = tutorials.lock().take_since(mark);
        for ask in asks {
            let now = Instant::now();
            let published = match ask {
                TutorialAsk::Selected { entity, guid } => {
                    let facts = self.reads.facts(entity, guid);
                    let mut state = tutorials.lock();
                    facts
                        .and_then(|f| select_tutorial(f, state.has_fired(id::PLAYERS)))
                        .and_then(|id| state.trigger(id, 0, now))
                }
                ask => tutorials.lock().apply(ask, now, &self.commands),
            };
            if let (Some(published), Some(sounds)) = (published, self.sounds.as_deref()) {
                fire_published(script, sounds, published);
            } else if let Some(published) = published {
                script.fire_event(
                    "TUTORIAL_TRIGGER",
                    vec![ScriptValue::Int(i64::from(published))],
                );
            }
        }
    }

    /// `FlagTutorial`'s `0x4b54c0` (`0x4b59dc`), `ClearTutorials`'s `0x4b55c0` (`0x4b5a00`) or
    /// `ResetTutorials`'s `0x4b5660` (`0x4b5a10`), its packet sent with any owed before it; or an
    /// engine step's `TriggerTutorial(id, 0)` (`0x4b5390`), its fire-once test, then the cue and
    /// `TUTORIAL_TRIGGER` in place, the bank free while the listeners run.
    pub(crate) fn apply(&mut self, script: &mut UiScript, call: TutorialCall) {
        let Some(tutorials) = self.tutorials.as_deref() else {
            return;
        };
        if let TutorialCall::Trigger(id) = call {
            let published = tutorials.lock().trigger(id, 0, Instant::now());
            if let Some(published) = published {
                if let Some(sounds) = self.sounds.as_deref() {
                    sounds.push_cue(TUTORIAL_POPUP_SOUND);
                }
                script.fire_event(
                    "TUTORIAL_TRIGGER",
                    vec![ScriptValue::Int(i64::from(published))],
                );
            }
            return;
        }
        let mut state = tutorials.lock();
        match call {
            TutorialCall::Flag(id) => state.acknowledge(id),
            TutorialCall::Clear => state.clear(),
            TutorialCall::Reset => state.reset(),
            TutorialCall::Trigger(_) => {}
        }
        state.flush(&self.commands);
    }
}

/// The world-enter bring-up: the banks zeroed, then the bank captured during the login handshake
/// applied if there was one.
fn on_world_enter(
    mut entered: MessageReader<EnteredWorldMessage>,
    tutorials: Res<Tutorials>,
    mut cascade: ResMut<WorldEnterCascade>,
) {
    let mut tutorials = tutorials.lock();
    for e in entered.read() {
        tutorials.world_enter();
        if let Some(bytes) = &e.tutorial_flags {
            tutorials.apply_flags(bytes);
        }
        cascade.entered_at = Some(Instant::now());
        cascade.moved = false;
        cascade.reset_watch = true;
    }
}

/// The Movement popup's silence window, and the descriptor watcher's reset, per world session.
#[derive(Resource, Default)]
pub(crate) struct WorldEnterCascade {
    entered_at: Option<Instant>,
    /// A movement input happened this session (the Movement auto-acknowledge site).
    pub(crate) moved: bool,
    /// The descriptor watcher's memory is owed a reset (a new world session).
    reset_watch: bool,
}

/// The Movement popup's silence window, the world frame's idle pass (`0x482ea0`, `0x483000`): its
/// tutorial when no movement input came in [`MOVEMENT_SILENCE`] since the world session began,
/// stamped at the frame pass, after the packets. The arrival's tutorial and the cascade's fire in
/// their places ([`fire_now`]).
fn run_world_enter_cascade(mut cascade: ResMut<WorldEnterCascade>, tutorials: Res<Tutorials>) {
    if let Some(at) = cascade.entered_at {
        if !cascade.moved && Instant::now().duration_since(at) >= MOVEMENT_SILENCE {
            cascade.entered_at = None;
            tutorials.trigger_now(id::MOVEMENT);
        }
    }
}

/// `FireTutorial 0x4b5390(id, 0)` in its caller's place: the arrival's ghost state's Death
/// (`0x5de9f0`) and the world-enter cascade's Welcome and Questgivers (`0x490a48`, `0x490a51`).
/// The fire-once test, then its `TUTORIAL_TRIGGER` with the host lent; with no VM the published
/// id waits on the lane for one.
pub(crate) fn fire_now(
    In(id): In<u32>,
    mut hosted_vm: crate::script_calls::HostedVm,
    tutorials: Res<Tutorials>,
    sounds: Res<crate::sound::MessageSounds>,
) {
    let Some(published) = tutorials.lock().trigger(id, 0, Instant::now()) else {
        return;
    };
    hosted_vm.run(|script| match script {
        Some(script) => fire_published(script, &sounds, published),
        None => tutorials.lock().push(TutorialAsk::Fire { published }),
    })
}

/// The world-input sites (`0x514840`), bundled for the controller, each in the input that makes
/// it: a movement input acknowledges Movement and arms the 10 s Targeting popup; a mouse-look
/// acknowledges Cameras.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct InputHooks<'w> {
    /// Optional, so a controller test's partial app runs without the tutorial plugin.
    tutorials: Option<Res<'w, Tutorials>>,
    commands: Option<Res<'w, NetCommands>>,
    cascade: Option<ResMut<'w, WorldEnterCascade>>,
}

impl InputHooks<'_> {
    /// A movement input this frame (`test ecx,0x1030`, taken as the forward and side axes):
    /// Movement acknowledged (`0x514a88`), then Targeting armed (`0x514a97`).
    pub(crate) fn moved(&mut self) {
        if let Some(c) = self.cascade.as_mut() {
            c.moved = true;
        }
        let (Some(tutorials), Some(commands)) =
            (self.tutorials.as_deref(), self.commands.as_deref())
        else {
            return;
        };
        tutorials.acknowledge_now(id::MOVEMENT, commands);
        // A delayed trigger fires nothing here: its timer holds it (`0x4b53e7`).
        let _ = tutorials
            .lock()
            .trigger(id::TARGETING, TARGETING_DELAY_MS, Instant::now());
    }

    /// A mouse-look this frame (`[ebp+8] & 3`, the two buttons, `0x514aa9`).
    pub(crate) fn mouselooked(&mut self) {
        if let (Some(tutorials), Some(commands)) =
            (self.tutorials.as_deref(), self.commands.as_deref())
        {
            tutorials.acknowledge_now(id::CAMERAS, commands);
        }
    }
}

fn on_world_leave(tutorials: Res<Tutorials>, mut cascade: ResMut<WorldEnterCascade>) {
    tutorials.lock().world_leave();
    cascade.entered_at = None;
}

pub(crate) struct TutorialPlugin;

impl Plugin for TutorialPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<Tutorials>()
            .init_resource::<WorldEnterCascade>()
            .add_lane("tutorials", peek_tutorials, fire_tutorials, None)
            .add_systems(
                Update,
                (
                    // The timers' callback ahead of the frame's input and packets.
                    tick_tutorial_timers.before(crate::ui_script::UiInput),
                    // The rest ahead of the feeds, so what each stamps fires in this frame's drain.
                    on_world_enter
                        .before(run_world_enter_cascade)
                        .before(crate::ui_script::UiFeed),
                    // The idle pass, after the packets.
                    run_world_enter_cascade
                        .after(benilla_world::schedule::WorldStage::Net)
                        .before(crate::ui_script::UiFeed),
                    watch_self
                        .after(on_world_enter)
                        .after(run_world_enter_cascade)
                        .after(crate::player::ControlRestore)
                        .before(crate::ui_script::UiFeed),
                    // Not before the in-game UI exists: the glue VM's queue is not the world's.
                    stamp_script_triggers
                        .in_set(crate::ui_script::UiFeed)
                        .run_if(crate::ui_script::ingame_ui_up),
                ),
            )
            .add_systems(
                OnExit(ClientState::InWorld),
                on_world_leave.in_set(crate::ui_script::SessionEnd::Clears),
            );
    }
}

/// The tutorial flags' packet handler.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::{TutorialAsk, Tutorials};
    use crate::net::NetHandlerApp;

    /// Register the handler, from [`super::TutorialPlugin`].
    pub(super) fn register(app: &mut App) {
        app.net_handler(SessionEventKind::TutorialFlags, on_flags);
    }

    /// `0x4b5700`: the copy at its packet's place on the tutorials lane.
    fn on_flags(In(ev): In<SessionEvent>, tutorials: Res<Tutorials>) {
        if let SessionEvent::TutorialFlags(bytes) = ev {
            tutorials.lock().push(TutorialAsk::Flags(bytes));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn banked() -> TutorialState {
        let mut t = TutorialState::default();
        t.apply_flags(&[0u8; 32]);
        t
    }

    /// StopAttack's tests (`0x5ecaf7`-`0x5ecb54`): each ratio strictly below half; Drink only
    /// for a mana user, and for one with no maximum mana at all (`0x5ecb37`); a 0 maximum
    /// health is no Food.
    #[test]
    fn the_stops_food_and_drink_tests_read_the_bytes_thresholds() {
        use benilla_protocol::ObjectFields;
        // Health 22, max 28; power1 23, max 29; `UNIT_FIELD_BYTES_0` 36, byte 3 the power type.
        let low = |health: u32, max_health: u32, power_type: u32, mana: u32, max_mana: u32| {
            let store = ObjectStore(ObjectFields::from_pairs(&[
                (22, health),
                (28, max_health),
                (23, mana),
                (29, max_mana),
                (36, power_type << 24),
            ]));
            LowVitals::of(Some(&store)).tutorials().collect::<Vec<_>>()
        };
        assert_eq!(low(49, 100, 0, 49, 100), [id::FOOD, id::DRINK]);
        assert!(
            low(50, 100, 0, 50, 100).is_empty(),
            "half is not below half"
        );
        assert_eq!(low(100, 100, 1, 0, 100), [] as [u32; 0], "rage: no Drink");
        assert_eq!(
            low(100, 100, 0, 0, 0),
            [id::DRINK],
            "no maximum mana: Drink"
        );
        assert!(low(0, 0, 1, 0, 0).is_empty(), "0/0 is NaN: no Food");
        assert!(LowVitals::of(None).tutorials().next().is_none());
    }

    /// No bank: silent. A clear bit: set in A only, the event owed, nothing sent. Twice: once.
    #[test]
    fn the_trigger_fires_once_per_bank_a_and_never_sends() {
        let now = Instant::now();
        let mut t = TutorialState::default();
        assert_eq!(t.trigger(id::WELCOME, 0, now), None, "no bank yet: silent");
        let mut t = banked();
        assert_eq!(
            t.trigger(id::WELCOME, 0, now),
            Some(id::WELCOME + 1),
            "published as id + 1"
        );
        assert_eq!(t.trigger(id::WELCOME, 0, now), None, "once");
        let (a, b) = t.bits();
        assert_eq!(bank_bit(a.unwrap(), id::WELCOME), Some(true));
        assert_eq!(
            bank_bit(b.unwrap(), id::WELCOME),
            Some(false),
            "bank B untouched"
        );
        assert!(t.sends.is_empty(), "the trigger sends nothing");
        assert_eq!(t.trigger(300, 0, now), None, "past the bank: a no-op");
    }

    /// The delayed leg holds the id until due; an acknowledge cancels it.
    #[test]
    fn a_delayed_trigger_waits_and_an_acknowledge_cancels_it() {
        let now = Instant::now();
        let mut t = banked();
        assert_eq!(t.trigger(id::TARGETING, TARGETING_DELAY_MS, now), None);
        assert!(
            t.tick(now + Duration::from_millis(9_999)).is_empty(),
            "not yet"
        );
        assert_eq!(
            t.tick(now + Duration::from_millis(10_000)),
            [id::TARGETING + 1]
        );

        let mut t = banked();
        t.trigger(id::TARGETING, TARGETING_DELAY_MS, now);
        t.acknowledge(id::TARGETING);
        assert!(
            t.tick(now + Duration::from_millis(20_000)).is_empty(),
            "the acknowledge cancelled the timer"
        );
        assert_eq!(t.sends, vec![TutorialSend::Flag(id::TARGETING)]);
    }

    /// The acknowledge: bank B gates, both banks set, one send; a second is silent; absent bank
    /// B is a no-op.
    #[test]
    fn the_acknowledge_sets_both_banks_and_sends_once() {
        let mut t = TutorialState::default();
        t.acknowledge(id::CHATTING);
        assert!(t.sends.is_empty(), "no bank: nothing");
        let mut t = banked();
        t.acknowledge(id::CHATTING);
        t.acknowledge(id::CHATTING);
        assert_eq!(t.sends, vec![TutorialSend::Flag(id::CHATTING)]);
        let (a, b) = t.bits();
        assert_eq!(bank_bit(a.unwrap(), id::CHATTING), Some(true));
        assert_eq!(bank_bit(b.unwrap(), id::CHATTING), Some(true));
        // Acknowledged first: the later trigger is silent (A ⊇ B).
        assert_eq!(t.trigger(id::CHATTING, 0, Instant::now()), None);
    }

    /// Clear sets every bit and flushes the timers; Reset clears every bit and keeps them.
    #[test]
    fn clear_and_reset_write_both_banks() {
        let now = Instant::now();
        let mut t = banked();
        t.trigger(id::TARGETING, TARGETING_DELAY_MS, now);
        t.clear();
        assert!(t.timers.is_empty(), "clear flushes the timers");
        let (a, b) = t.bits();
        assert!(a.unwrap().iter().all(|&x| x == 0xFF) && b.unwrap().iter().all(|&x| x == 0xFF));
        t.reset();
        t.trigger(id::TARGETING, TARGETING_DELAY_MS, now);
        assert_eq!(
            t.timers.len(),
            1,
            "a cleared bank lets the trigger through again"
        );
        t.reset();
        assert_eq!(t.timers.len(), 1, "reset does NOT flush the timers");
        let (a, b) = t.bits();
        assert!(a.unwrap().iter().all(|&x| x == 0) && b.unwrap().iter().all(|&x| x == 0));
        assert_eq!(
            t.sends,
            vec![
                TutorialSend::Clear,
                TutorialSend::Reset,
                TutorialSend::Reset
            ]
        );
    }

    /// World enter zeroes present banks and keeps their size; the packet fills both.
    #[test]
    fn world_enter_zeroes_and_the_packet_fills() {
        let mut t = TutorialState::default();
        t.world_enter();
        assert!(t.fire_once.is_none(), "no bank: nothing to zero");
        t.apply_flags(&[0xFFu8; 4]);
        t.world_enter();
        let (a, b) = t.bits();
        assert_eq!(a.unwrap(), &[0u8; 4]);
        assert_eq!(b.unwrap(), &[0u8; 4]);
        assert_eq!(bank_bit(a.unwrap(), 31), Some(false));
        assert_eq!(bank_bit(a.unwrap(), 32), None, "past a 4-byte bank");
    }

    /// The flag tutorials are field watchers' (`crate::field_watch`), which a first create skips:
    /// the self descriptor's poll raises none, whatever its flags became since it last looked.
    #[test]
    fn the_self_poll_raises_no_flag_tutorial() {
        use benilla_protocol::messages::ObjectType;
        use benilla_protocol::ObjectFields;
        let mut app = App::new();
        app.init_resource::<Player>()
            .init_resource::<WorldEnterCascade>()
            .init_resource::<Tutorials>()
            .add_systems(Update, watch_self);
        let store = |pairs: &[(u16, u32)]| {
            ObjectStore(ObjectFields::from_pairs(pairs).into_created(ObjectType::Player))
        };
        let me = app.world_mut().spawn((SelfPlayer, store(&[(34, 10)]))).id();
        app.update();
        app.world_mut().entity_mut(me).insert(store(&[
            (34, 10),
            (190, 0x30),
            (46, 0x1000),
            (194, 1 << 24),
        ]));
        app.update();
        let asked = app.world().resource::<Tutorials>().take_asks();
        assert!(asked.is_empty(), "{asked:?}");
    }

    /// The engine's ask beside a script, a passing turn-in test's Quest Completion (`0x4df792`),
    /// is stamped on the tutorials lane, whose fire-once test publishes it as 34.
    #[test]
    fn a_completable_quest_triggers_the_quest_completion_tutorial() {
        let mut app = App::new();
        app.init_resource::<Tutorials>()
            .add_systems(Update, stamp_script_triggers);
        let mut script = UiScript::new().unwrap();
        let mut q = crate::ui_quest_log::fixture::quest(7, "Deliver the Message", 1);
        q.record.objectives[0] = crate::ui_quest_log::fixture::kill(40, 3);
        q.slot.counters[0] = 3;
        crate::ui_quest_log::fixture::seat(&mut script, &[q]);
        script
            .run("local _, _, _, _, _, c = GetQuestLogTitle(2) assert(c == 1)")
            .unwrap();
        app.insert_non_send_resource(script);
        app.world_mut()
            .resource::<Tutorials>()
            .lock()
            .apply_flags(&[0u8; 32]);
        app.update();
        assert_eq!(
            app.world().resource::<Tutorials>().take_fired(),
            [34],
            "the engine's 0x21, published as `TUTORIAL_TITLE34`"
        );
    }

    /// The tutorial plugin with what its lane's firing reads, around one VM recording
    /// `TUTORIAL_TRIGGER`, in the world.
    fn plugin_app() -> App {
        let mut app = App::new();
        app.add_plugins(TutorialPlugin)
            .init_resource::<crate::net::Reputations>()
            .init_resource::<crate::items::Items>()
            .init_resource::<crate::sound::MessageSounds>()
            .init_resource::<crate::ui_trainer::TrainerOpen>()
            .init_resource::<crate::ui_merchant::MerchantOpen>()
            .init_resource::<crate::ui_taxi::TaxiState>()
            .init_resource::<Player>()
            .add_message::<EnteredWorldMessage>();
        let (tx, _rx) = crossbeam_channel::unbounded();
        app.insert_resource(crate::net::NetCommands(tx));
        app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
        let script = UiScript::new().expect("a VM");
        script
            .run(
                "SEEN = {} local f = CreateFrame('Frame') \
                 f:RegisterEvent('TUTORIAL_TRIGGER') \
                 f:SetScript('OnEvent', function() table.insert(SEEN, arg1) end)",
            )
            .unwrap();
        app.insert_non_send_resource(script);
        app
    }

    /// `TUTORIAL_TRIGGER` is fired once, so one published to a frameless VM is gone for the
    /// session: the lane holds its asks until an interface is up.
    #[test]
    fn the_world_enter_tutorials_are_not_fired_into_a_ui_less_vm() {
        let mut app = plugin_app();
        // The deferral window: in the world on the wire, the entry load still owed.
        app.insert_resource(crate::ui_script::PendingEntryUiLoad);
        {
            let t = app.world().resource::<Tutorials>();
            t.lock().apply_flags(&[0u8; 32]);
            t.trigger_now(id::WELCOME);
        }
        app.update();
        assert_eq!(
            app.world().resource::<Tutorials>().lock().lane.len(),
            1,
            "the window must not publish: the take is one-way"
        );
        assert!(seen(&mut app).is_empty());

        // The interface comes up; the same frame delivers what was waiting.
        app.world_mut()
            .remove_resource::<crate::ui_script::PendingEntryUiLoad>();
        app.update();
        assert_eq!(seen(&mut app), [i64::from(id::WELCOME + 1)]);
        assert!(
            app.world().resource::<Tutorials>().lock().lane.is_empty(),
            "…and once there is an interface, it is published exactly once"
        );
    }

    /// The active player's arrival runs its ghost state (`0x5dea50` → `0x5deb2a` → `0x5de9c0`)
    /// before the world-enter cascade (`0x5deb60` → `0x4908c0`): arriving with `PLAYER_FLAGS`
    /// bit 0x10 set fires Death (0x18, `0x5de9eb`-`0x5de9f0`) ahead of Welcome and Questgivers,
    /// published as their number plus one; arriving alive fires no Death.
    #[test]
    fn a_ghost_arrival_fires_death_ahead_of_the_cascades_tutorials() {
        const PLAYER_FLAGS: u16 = 190;
        for (flags, published) in [(0x10, vec![0x19, 0x2a, 0x01]), (0, vec![0x2a, 0x01])] {
            let mut app = App::new();
            app.add_plugins(TutorialPlugin)
                .init_resource::<crate::net::Reputations>()
                .init_resource::<crate::items::Items>()
                .init_resource::<crate::sound::MessageSounds>()
                .init_resource::<crate::ui_trainer::TrainerOpen>()
                .init_resource::<crate::ui_merchant::MerchantOpen>()
                .init_resource::<crate::ui_taxi::TaxiState>()
                .init_resource::<Player>()
                .add_plugins(crate::ui_world_enter::WorldEnterPlugin)
                .add_message::<EnteredWorldMessage>();
            let (tx, _rx) = crossbeam_channel::unbounded();
            app.insert_resource(crate::net::NetCommands(tx));
            app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
            let script = UiScript::new().expect("a VM");
            script
                .run(
                    "SEEN = {} local f = CreateFrame('Frame') \
                     f:RegisterEvent('TUTORIAL_TRIGGER') \
                     f:SetScript('OnEvent', function() table.insert(SEEN, arg1) end)",
                )
                .unwrap();
            app.insert_non_send_resource(script);
            app.world_mut().write_message(EnteredWorldMessage {
                tutorial_flags: Some(vec![0u8; 32]),
            });
            app.world_mut().spawn((
                SelfPlayer,
                ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(
                    PLAYER_FLAGS,
                    flags,
                )])),
            ));
            // The create's arrival (`0x5dec7b`), as our own create's handler pushes it.
            crate::ui_world_enter::push_arrival(app.world_mut());
            app.update();
            let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
            s.resolve();
            assert_eq!(
                s.eval::<Vec<i64>>("return SEEN").unwrap(),
                published,
                "flags {flags:#x}"
            );
        }
    }

    /// The cascade `0x4908c0` fires Welcome (`0x490a48`) and Questgivers (`0x490a51`) on every
    /// pass: a cross-map worldport's create (`0x5deb60`) and a `/reload` (`0x490168`) as well as
    /// the login, with the banks zeroed only at the login's bring-up (`0x4b5330`). So after
    /// Reset Tutorials, the next cascade with no world entry pops both again.
    #[test]
    fn every_world_enter_cascade_fires_welcome_and_questgivers() {
        let mut app = App::new();
        app.add_plugins(TutorialPlugin)
            .init_resource::<crate::net::Reputations>()
            .init_resource::<crate::items::Items>()
            .init_resource::<crate::sound::MessageSounds>()
            .init_resource::<crate::ui_trainer::TrainerOpen>()
            .init_resource::<crate::ui_merchant::MerchantOpen>()
            .init_resource::<crate::ui_taxi::TaxiState>()
            .init_resource::<Player>()
            .add_plugins(crate::ui_world_enter::WorldEnterPlugin)
            .add_message::<EnteredWorldMessage>();
        let (tx, _rx) = crossbeam_channel::unbounded();
        app.insert_resource(crate::net::NetCommands(tx));
        app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
        let script = UiScript::new().expect("a VM");
        script
            .run(
                "SEEN = {} local f = CreateFrame('Frame') \
                 f:RegisterEvent('TUTORIAL_TRIGGER') \
                 f:SetScript('OnEvent', function() table.insert(SEEN, arg1) end)",
            )
            .unwrap();
        app.insert_non_send_resource(script);
        app.world_mut().spawn((
            SelfPlayer,
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[])),
        ));
        // Mid-session: the login's flags long applied and both tutorials since reset.
        app.world()
            .resource::<Tutorials>()
            .lock()
            .apply_flags(&[0u8; 32]);
        app.update();
        let seen = |app: &mut App| {
            let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
            s.resolve();
            s.eval::<Vec<i64>>("return SEEN").unwrap()
        };
        assert!(seen(&mut app).is_empty(), "no cascade, nothing");
        app.world_mut()
            .resource_mut::<crate::ui_world_enter::ArrivalLane>()
            .push(crate::ui_world_enter::Entry::Reload);
        app.update();
        assert_eq!(
            seen(&mut app),
            [0x2a, 0x01],
            "a `/reload` or a port's cascade"
        );
    }

    /// `SetSelection`'s ladder (`0x493789`-`0x49383c`), first match: another player is Players
    /// until its bit is set, then falls through; Grouping needs a cooperative player and the
    /// active player at level 5 (`0x4937f2 cmp [eax+0x70],5`); then Combat Mode, then Travel.
    #[test]
    fn the_selection_ladder_follows_the_bytes() {
        let other = SelectFacts {
            player: true,
            cooperates: true,
            my_level: 5,
            ..Default::default()
        };
        assert_eq!(select_tutorial(other, false), Some(id::PLAYERS));
        assert_eq!(
            select_tutorial(other, true),
            Some(id::GROUPING),
            "Players fired: on to Grouping"
        );
        let low = SelectFacts {
            my_level: 4,
            attackable: true,
            ..other
        };
        assert_eq!(select_tutorial(low, true), Some(id::COMBAT_MODE), "below 5");
        let me = SelectFacts { me: true, ..other };
        assert_eq!(
            select_tutorial(me, false),
            Some(id::GROUPING),
            "our own player skips Players"
        );
        let npc = SelectFacts {
            flight_master: true,
            ..Default::default()
        };
        assert_eq!(select_tutorial(npc, false), Some(id::TRAVEL));
        let foe = SelectFacts {
            attackable: true,
            flight_master: true,
            ..Default::default()
        };
        assert_eq!(
            select_tutorial(foe, false),
            Some(id::COMBAT_MODE),
            "attack first"
        );
        assert_eq!(select_tutorial(SelectFacts::default(), false), None);
    }

    /// The lane's firing over one VM recording `TUTORIAL_TRIGGER`, with what the selection ladder
    /// reads, run alone on what the lane holds.
    fn recording_lane() -> App {
        let mut app = App::new();
        app.init_resource::<Tutorials>()
            .init_resource::<crate::sound::MessageSounds>()
            .init_resource::<crate::net::Reputations>()
            .init_resource::<crate::net::GuidIndex>()
            .insert_resource(crate::net::SelfGuid(Some(0x10)));
        let (tx, _rx) = crossbeam_channel::unbounded();
        app.insert_resource(crate::net::NetCommands(tx));
        let script = UiScript::new().unwrap();
        script
            .run(
                "SEEN = {} local f = CreateFrame('Frame') \
                 f:RegisterEvent('TUTORIAL_TRIGGER') \
                 f:SetScript('OnEvent', function() table.insert(SEEN, arg1) end)",
            )
            .unwrap();
        app.insert_non_send_resource(script);
        app.world()
            .resource::<Tutorials>()
            .lock()
            .apply_flags(&[0u8; 32]);
        app
    }

    fn fire_lane(app: &mut App) {
        use bevy::ecs::system::RunSystemOnce;
        app.world_mut()
            .run_system_once_with(fire_tutorials, Until::all())
            .expect("the tutorials lane fires");
    }

    fn seen(app: &mut App) -> Vec<i64> {
        let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
        s.resolve();
        s.eval::<Vec<i64>>("return SEEN").unwrap()
    }

    /// The selections go up the ladder at their places on the lane, in order: another player is
    /// Players (`0x4937be`), an attackable creature Combat Mode (`0x49381a`), a flight master
    /// that cannot be attacked Travel (`0x49383c`), each published as its number plus one.
    #[test]
    fn a_selection_fires_its_rung_of_the_ladder() {
        use benilla_protocol::ObjectFields;
        let mut app = recording_lane();
        let mut unit = |guid: u64, pairs: &[(u16, u32)]| {
            let world = app.world_mut();
            let e = world
                .spawn((
                    crate::net::Guid(guid),
                    ObjectStore(ObjectFields::from_pairs(pairs)),
                ))
                .id();
            world
                .resource_mut::<crate::net::GuidIndex>()
                .0
                .insert(guid, e);
            (e, guid)
        };
        // Type 0x19 a player, 0x9 a unit; health 22, level 34, flags 46 (0x8 player-controlled,
        // 0x100 immune to players), NPC flags 147.
        let (me, _) = unit(0x10, &[(2, 0x19), (22, 100), (46, 0x8), (34, 10)]);
        let player = unit(0x11, &[(2, 0x19), (22, 100)]);
        let mob = unit(0xF130_0000_0000_0031, &[(2, 0x9), (22, 100)]);
        let flight = unit(
            0xF130_0000_0000_0032,
            &[(2, 0x9), (22, 100), (46, 0x100), (147, 0x8)],
        );
        app.world_mut().entity_mut(me).insert(SelfPlayer);
        {
            let tutorials = app.world().resource::<Tutorials>();
            for (e, g) in [player, mob, flight] {
                tutorials.selected(e, g);
            }
        }
        fire_lane(&mut app);
        assert_eq!(seen(&mut app), [0x13, 0x05, 0x23]);
    }

    fn template(
        edit: impl FnOnce(&mut benilla_protocol::messages::ItemInfo),
    ) -> benilla_protocol::messages::ItemInfo {
        let mut info = crate::items::test_template("Item");
        edit(&mut info);
        info
    }

    /// The item-received sites in the function's order (`0x491cec`-`0x491d5b`): Backpack on any
    /// item, Using Items for a template with an on-use spell (`0x491d00`-`0x491d1e`, trigger 0),
    /// Bags for a container (class 1, `0x491d4f`); an on-equip spell is no use.
    #[test]
    fn a_received_on_use_item_and_container_fire_using_items_and_bags() {
        use benilla_protocol::messages::ItemSpellEntry;
        let spell = |trigger: u32| ItemSpellEntry {
            index: 0,
            spell_id: 100,
            trigger,
            charges: 0,
            cooldown_ms: -1,
            category: 0,
            category_cooldown_ms: -1,
        };
        let ring = template(|i| i.spells = vec![spell(1)]);
        let potion = template(|i| i.spells = vec![spell(0)]);
        let bag = template(|i| i.class = 1);
        assert_eq!(item_tutorials(1, false, &ring, None), [id::BACKPACK]);
        assert_eq!(
            item_tutorials(2, false, &potion, None),
            [id::BACKPACK, id::USING_ITEMS]
        );
        assert_eq!(
            item_tutorials(3, false, &bag, None),
            [id::BACKPACK, id::BAGS]
        );
        assert_eq!(
            item_tutorials(HEARTHSTONE, true, &ring, None),
            [id::BACKPACK, id::KEYRINGS, id::HEARTHSTONES],
            "the keyring band, then the hearthstone's entry"
        );
    }

    /// Equippable Items fires on a set bit among the first 23 of the slot mask `0x809200`
    /// indexed by the inventory type (`0x491d23`-`0x491d4a`). The mask is 0 for types 0, 24
    /// (ammo) and 27 (quiver), so those raise no prompt; a bag (type 18, `0x780000`) does. A
    /// ranged weapon is Ranged Weapons last, but not for a hunter (`0x491ebc`).
    #[test]
    fn equippable_items_reads_the_slot_mask_so_ammo_and_quivers_raise_none() {
        for inventory_type in [0, 24, 27] {
            let info = template(|i| i.inventory_type = inventory_type);
            assert_eq!(
                item_tutorials(1, false, &info, None),
                [id::BACKPACK],
                "only Backpack: no equippable prompt for type {inventory_type}"
            );
        }
        let bag = template(|i| i.inventory_type = 18);
        assert_eq!(
            item_tutorials(4, false, &bag, None),
            [id::BACKPACK, id::EQUIPPABLE_ITEMS],
            "a bag is worn in a bag slot"
        );
        let bow = template(|i| i.inventory_type = INVTYPE_RANGED);
        assert_eq!(
            item_tutorials(5, false, &bow, Some(1)),
            [id::BACKPACK, id::EQUIPPABLE_ITEMS, id::RANGED_WEAPONS]
        );
        assert_eq!(
            item_tutorials(5, false, &bow, Some(CLASS_HUNTER)),
            [id::BACKPACK, id::EQUIPPABLE_ITEMS]
        );
    }
}
