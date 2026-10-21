//! The social session: the friend list, the ignore list, `/who`, and the system lines they print.
//! The reference's `FriendList` (`0xc28168`) holds 50 friend slots of a guid, the name copied out
//! of the name cache (`0x55f080`), the presence fields, and the selection as a guid (`+0x648`); it
//! is built at each world entry (`0x5adc50`) and freed at its teardown (`0x5ae2d0`).
//!
//! A name a function looks up and misses counts as pending (`+0x640`), and the cache's callback
//! (`0x5ace70`) finishes the work once it is answered: the list sorts and signals
//! `FRIENDLIST_UPDATE` only with nothing pending (`0x5ae850`). Every friend and ignore result
//! prints to the chat frame: `HandleResult` (`0x5acab0`) shows catalog ids `0x104..0x114` through
//! `DisplayError` (`0x496720`), all of kind 0 (`0xb4b498`), the chat composer (`0x49a870`).

use benilla_protocol::messages::{friend_result, friend_status, FriendEntry, FriendStatusUpdate};
use bevy::prelude::*;

use crate::console::ConsoleCommandApp;
use crate::net::ClientCommand;
use crate::ui_action::UiError;
use crate::ui_script::UiFeed;

#[cfg(test)]
mod call_tests;
mod feed;
mod query;
mod steps;
mod whois;

pub(crate) use query::parse as who_query;
pub(crate) use steps::SocialSteps;

/// The friend slots (`0x32`).
const FRIEND_SLOTS: usize = 50;
/// The ignore slots (`0x19`), 8 bytes each from `+0x650`.
const IGNORE_SLOTS: usize = 25;

/// The name cache (`0xc0e228`) as the list's functions reach it, through `0x55f080`.
pub(crate) trait Names {
    /// The lookup with a callback: the cached name, or `None` after asking the server.
    fn lookup(&self, guid: u64) -> Option<String>;
    /// The lookup with none: the cached name, never asking.
    fn read(&self, guid: u64) -> Option<String>;
}

/// No name cached, for a test of a function that only seats a list.
#[cfg(test)]
pub(crate) struct NoNames;

#[cfg(test)]
impl Names for NoNames {
    fn lookup(&self, _: u64) -> Option<String> {
        None
    }
    fn read(&self, _: u64) -> Option<String> {
        None
    }
}

/// The two lists as a signal left them, which its event's listeners read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Lists {
    slots: Vec<FriendSlot>,
    friends_pending: u32,
    ignores: Vec<u64>,
    /// Each ignore slot's name as the cache held it, read with no callback (`0x5ad4e5`).
    ignore_names: Vec<Option<String>>,
    ignores_pending: u32,
    /// The name of the guid `GetIgnoreName(26)` reads, the pending count (`+0x718`).
    after_ignores_name: Option<String>,
}

/// Which list a signal is about: `FRIENDLIST_UPDATE` (`0x15f`, `0x5ae867`) or
/// `IGNORELIST_UPDATE` (`0x160`, `0x5ae92d`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ListEvent {
    Friends,
    Ignores,
}

/// What a handler did that Lua hears, in the order it did it: a list event signalled, or a line
/// shown (`0x496720`).
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Heard {
    Event(ListEvent),
    Line(UiError),
}

/// One thing a handler did that Lua hears, with the lists as it left them: the feed runs its
/// listeners on those lists, as the reference runs them inside the handler.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Step {
    pub(super) lists: Lists,
    pub(super) heard: Heard,
}

/// One friend slot, the 32 bytes at `+0x20·k`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct FriendSlot {
    /// `+0`, written 0 or 1 only.
    online: bool,
    /// `+1`, the status flags: 2 away, 4 busy.
    status: u8,
    /// `+4`, the slot's own copy of the name, null until a lookup finds one.
    name: Option<String>,
    /// `+8`; 0 is an empty slot.
    guid: u64,
    /// `+0x10`, `+0x14`, `+0x18`.
    level: u32,
    class: u32,
    area: u32,
}

/// A name lookup that missed, waiting for the cache's callback: the friends' `0x5ace70` or the
/// ignores' `0x5acf40`, with the message the lookup passed, `None` for `0x1d1`, which only stores
/// the name.
#[derive(Clone, Debug, PartialEq, Eq)]
struct NameWait {
    guid: u64,
    line: Option<&'static str>,
    ignore: bool,
}

/// The social session mirror: filled by the social handlers and the calls, read by the feed,
/// cleared at the world's teardown.
#[derive(Resource)]
pub(crate) struct SocialState {
    slots: Vec<FriendSlot>,
    /// `+0x640`, the friend names pending; never reset, so a lookup no answer reaches holds every
    /// later sort.
    friends_pending: u32,
    /// The selected friend's guid (`+0x648`); 0 is none.
    selected_friend: u64,
    /// The 25 ignore guids (`+0x650`); 0 is an empty slot.
    ignores: Vec<u64>,
    /// `+0x718`, the ignore names pending, as `+0x640` for the friends.
    ignores_pending: u32,
    /// The selected ignore's guid (`+0x720`); 0 is none.
    selected_ignore: u64,
    /// The last `/who` answer; `who_total` is the server's full match count.
    who: Vec<benilla_protocol::messages::WhoEntry>,
    who_total: u32,
    /// The `/who` sort chain (`SortWho`'s seven `{key, dir}` slots at `0xc2817c`), pushed with the
    /// rows so the binding can re-sort synchronously.
    who_sort: benilla_ui::script::WhoSortChain,
    /// `SetWhoToUI`, which the Who frame sets on show and clears on hide.
    who_to_ui: bool,
    /// The lookups that missed, in the order made.
    waits: Vec<NameWait>,
    /// The list events and lines since the last feed, in order, each with the lists it left.
    pub(super) steps: Vec<Step>,
    who_dirty: bool,
}

impl Default for SocialState {
    fn default() -> Self {
        Self {
            slots: vec![FriendSlot::default(); FRIEND_SLOTS],
            friends_pending: 0,
            selected_friend: 0,
            ignores: vec![0; IGNORE_SLOTS],
            ignores_pending: 0,
            selected_ignore: 0,
            who: Vec::new(),
            who_total: 0,
            who_sort: Default::default(),
            who_to_ui: false,
            waits: Vec::new(),
            steps: Vec::new(),
            who_dirty: false,
        }
    }
}

impl SocialState {
    /// Drop the session's state, as the teardown frees the list (`0x5ae2d0`) and the next world
    /// entry builds an empty one. The `/who` sort chain survives, since the reference initialises
    /// it once per process (`0x5adc50`, from `0x401666`), never at login.
    pub(crate) fn clear_session(&mut self) {
        *self = Self {
            who_sort: std::mem::take(&mut self.who_sort),
            ..Self::default()
        };
    }

    /// Is `guid` in an ignore slot? The reference's `IsIgnored` (`0x5ae5a0`): an ignored player's
    /// chat and text emotes drop silently (`0x49dc36`), and their duel is declined (`0x4d4a33`).
    pub(crate) fn is_ignored(&self, guid: u64) -> bool {
        guid != 0 && self.ignores.contains(&guid)
    }

    /// The lists as they stand, the ignores' names read from the cache.
    pub(super) fn lists(&self, names: &dyn Names) -> Lists {
        Lists {
            slots: self.slots.clone(),
            friends_pending: self.friends_pending,
            ignores: self.ignores.clone(),
            ignore_names: self.ignores.iter().map(|&g| names.read(g)).collect(),
            ignores_pending: self.ignores_pending,
            after_ignores_name: names.read(u64::from(self.ignores_pending)),
        }
    }

    /// Signal `event` with the lists as they stand, for the feed to fire in order.
    fn signal(&mut self, event: ListEvent, names: &dyn Names) {
        self.step(Heard::Event(event), names);
    }

    /// Show `line` with the lists as they stand, for the feed to show in order.
    fn line(&mut self, line: UiError, names: &dyn Names) {
        self.step(Heard::Line(line), names);
    }

    fn step(&mut self, heard: Heard, names: &dyn Names) {
        let lists = self.lists(names);
        self.steps.push(Step { lists, heard });
    }

    /// Trade the lists for `lists`: the feed swaps a step's lists in around its listeners, so the
    /// verbs they call act on the list their signal left (`FriendsFrame.lua`'s `FriendsList_Update`
    /// selects friend 1 inside the event), and swaps them back after. No verb writes the lists.
    pub(super) fn swap_lists(&mut self, lists: &mut Lists) {
        std::mem::swap(&mut self.slots, &mut lists.slots);
        std::mem::swap(&mut self.friends_pending, &mut lists.friends_pending);
        std::mem::swap(&mut self.ignores, &mut lists.ignores);
        std::mem::swap(&mut self.ignores_pending, &mut lists.ignores_pending);
    }

    /// Is `guid` in a friend slot? The reference's `IsFriend` (`0x5ae810`), which keeps the guild
    /// sign-on line from announcing a friend twice; it compares all 50 slots.
    pub(crate) fn is_friend(&self, guid: u64) -> bool {
        guid != 0 && self.find(guid).is_some()
    }

    /// The first of the 50 slots holding `guid`, empty ones included, so guid 0 finds the first
    /// empty slot (`0x5acc20`, `0x5aead1`, `0x5aeb41`).
    fn find(&self, guid: u64) -> Option<usize> {
        self.slots.iter().position(|s| s.guid == guid)
    }

    /// `FindOrAddFriendSlot` (`0x5aea60`): the slot holding `guid`, else the first empty one, which
    /// takes the guid and an online byte of 0; the scan stops at that empty slot. A full list
    /// without the guid writes it before the object in the reference (`0x5aea92`) and finds none.
    fn find_or_add(&mut self, guid: u64) -> Option<usize> {
        for (k, slot) in self.slots.iter_mut().enumerate() {
            if slot.guid == guid {
                return Some(k);
            }
            if slot.guid == 0 {
                slot.guid = guid;
                slot.online = false;
                return Some(k);
            }
        }
        None
    }

    /// `SetFriendStatus` (`0x5aeb30`): the online byte of the slot holding `guid`.
    fn set_online(&mut self, guid: u64, online: bool) {
        if let Some(k) = self.find(guid) {
            self.slots[k].online = online;
        }
    }

    /// `SetFriendName` (`0x5aeac0`): the name of the slot holding `guid`.
    fn set_name(&mut self, guid: u64, name: &str) {
        if let Some(k) = self.find(guid) {
            self.slots[k].name = Some(name.to_string());
        }
    }

    /// `SortFriends` (`0x5ae850`): the CRT `qsort` of the 50 slots with `CompareFriends`
    /// (`0x5ae870`), then `FRIENDLIST_UPDATE` (`0x5ae867`).
    fn sort_friends(&mut self, names: &dyn Names) {
        benilla_ui::crt::qsort(&mut self.slots, compare_friends);
        self.signal(ListEvent::Friends, names);
    }

    /// `SortIgnores` (`0x5ae910`): the CRT `qsort` of the 25 guids with `CompareIgnoredByName`
    /// (`0x5ae940`), then `IGNORELIST_UPDATE` (`0x5ae92d`).
    fn sort_ignores(&mut self, names: &dyn Names) {
        benilla_ui::crt::qsort(&mut self.ignores, |&a, &b| compare_ignores(a, b, names));
        self.signal(ListEvent::Ignores, names);
    }

    /// `DecrementPendingFriendNames` (`0x5aeb70`) and `DecrementPendingIgnoreNames` (`0x5aeb90`):
    /// a pending count above 0 drops by one, and its list sorts when it reaches 0.
    fn decrement_pending(&mut self, ignore: bool, names: &dyn Names) {
        let pending = if ignore {
            &mut self.ignores_pending
        } else {
            &mut self.friends_pending
        };
        if *pending == 0 {
            return;
        }
        *pending -= 1;
        if *pending == 0 {
            if ignore {
                self.sort_ignores(names);
            } else {
                self.sort_friends(names);
            }
        }
    }

    /// Look `guid`'s name up with a list's callback, the miss counted in that list's pending count
    /// and its callback waited for.
    fn look_up(
        &mut self,
        guid: u64,
        line: Option<&'static str>,
        ignore: bool,
        names: &dyn Names,
    ) -> Option<String> {
        let name = names.lookup(guid);
        if name.is_none() {
            let pending = if ignore {
                &mut self.ignores_pending
            } else {
                &mut self.friends_pending
            };
            *pending = pending.wrapping_add(1);
            self.waits.push(NameWait { guid, line, ignore });
        }
        name
    }

    /// `AddIgnoreSlot` (`0x5af5d0`): `guid` into the first empty slot, with no test for a slot
    /// holding it already, or dropped when all 25 are taken (`0x5af5eb`, `0x5af5f0`). With
    /// `look` its name is looked up (`0x5af627`) and the ignores sort with nothing pending
    /// (`0x5af636`).
    fn add_ignore_slot(&mut self, guid: u64, look: bool, names: &dyn Names) {
        let Some(k) = self.ignores.iter().position(|&g| g == 0) else {
            return;
        };
        self.ignores[k] = guid;
        if !look {
            return;
        }
        self.look_up(guid, None, true, names);
        if self.ignores_pending == 0 {
            self.sort_ignores(names);
        }
    }

    /// `0x5af650`, result 16's: the first slot holding `guid` zeroed, and the ignores sorted when
    /// nothing is pending (`0x5af691`); the selection stays.
    fn remove_ignore(&mut self, guid: u64, names: &dyn Names) {
        if let Some(k) = self.ignores.iter().position(|&g| g == guid) {
            self.ignores[k] = 0;
            if self.ignores_pending == 0 {
                self.sort_ignores(names);
            }
        }
    }

    /// `SMSG_FRIEND_LIST` (`0x5ae350`): every slot's name freed (`0x5ae366`-`0x5ae388`), then up to
    /// 50 records into the slots from the first, a status of 0 storing zeros for the area, level
    /// and class (`0x5ae415`); the slots past the records keep their guids. Each name is copied
    /// from the cache (`0x5ae438`-`0x5ae451`) or counted pending (`0x5ae459`), and the list sorts
    /// only with nothing pending (`0x5ae475`-`0x5ae481`).
    fn apply_friend_list(&mut self, friends: &[FriendEntry], names: &dyn Names) {
        for slot in &mut self.slots {
            slot.name = None;
        }
        for (k, entry) in friends.iter().take(FRIEND_SLOTS).enumerate() {
            let on = entry.status != friend_status::OFFLINE;
            self.slots[k] = FriendSlot {
                online: on,
                status: entry.status,
                name: None,
                guid: entry.guid,
                level: if on { entry.level } else { 0 },
                class: if on { entry.class } else { 0 },
                area: if on { entry.area } else { 0 },
            };
            let name = self.look_up(entry.guid, None, false, names);
            self.slots[k].name = name;
        }
        if self.friends_pending == 0 {
            self.sort_friends(names);
        }
    }

    /// Seat the ignore list directly, for tests of the code that consults it.
    #[cfg(test)]
    pub(crate) fn set_ignores_for_test(&mut self, guids: Vec<u64>) {
        self.apply_ignore_list(&guids, &NoNames);
    }

    /// `SMSG_IGNORE_LIST` (`0x5af530`): the 25 slots zeroed (`0x5af548`), then every guid the
    /// count names, with no cap: each seated by `AddIgnoreSlot` without a lookup (`0x5af578`), which
    /// drops one past the 25th, then looked up with the ignores' callback (`0x5af59a`), a miss
    /// counted (`0x5af5a3`). The ignores sort only with nothing pending (`0x5af5b2`).
    fn apply_ignore_list(&mut self, guids: &[u64], names: &dyn Names) {
        self.ignores = vec![0; IGNORE_SLOTS];
        for &guid in guids {
            self.add_ignore_slot(guid, false, names);
            self.look_up(guid, None, true, names);
        }
        if self.ignores_pending == 0 {
            self.sort_ignores(names);
        }
    }

    /// `SMSG_FRIEND_STATUS`, `HandleResult` (`0x5acab0`) through its table (`0x5ace1c`): a result
    /// that names no one shows its line alone; the others act on a list, then look the name up
    /// with their line as the callback's message. A cached name is stored for an add
    /// (`0x5acdbd`), the list sorts and the line shows with the name (`0x5acdd5`-`0x5ace08`); a
    /// miss counts as pending and returns unsignalled (`0x5acba6`-`0x5acdba`).
    fn apply_friend_status(&mut self, update: FriendStatusUpdate, names: &dyn Names) {
        let FriendStatusUpdate {
            result,
            guid,
            online,
        } = update;
        let key = result_key(result);
        let fill = |slot: &mut FriendSlot| {
            // The tail's status byte and three dwords (`0x5acb04`-`0x5acb33`); a packet without
            // one reads zeros here, where the reference reads stale stack.
            let tail = online.unwrap_or(benilla_protocol::messages::FriendOnline {
                status: 0,
                area: 0,
                level: 0,
                class: 0,
            });
            slot.status = tail.status;
            slot.area = tail.area;
            slot.level = tail.level;
            slot.class = tail.class;
        };
        let ignore = match result {
            friend_result::ONLINE => {
                if let Some(k) = self.find(guid) {
                    fill(&mut self.slots[k]);
                }
                self.set_online(guid, true);
                false
            }
            friend_result::OFFLINE => {
                self.set_online(guid, false);
                false
            }
            // `ClearFriendSlot` (`0x5aea00`): the guid and the name; the rest of the slot stays.
            friend_result::REMOVED => {
                if let Some(k) = self.find(guid) {
                    self.slots[k].guid = 0;
                    self.slots[k].name = None;
                }
                false
            }
            friend_result::ADDED_ONLINE => {
                if let Some(k) = self.find_or_add(guid) {
                    fill(&mut self.slots[k]);
                }
                self.set_online(guid, true);
                false
            }
            friend_result::ADDED_OFFLINE => {
                self.find_or_add(guid);
                self.set_online(guid, false);
                false
            }
            friend_result::ALREADY => {
                self.find_or_add(guid);
                false
            }
            friend_result::IGNORE_ALREADY => true,
            friend_result::IGNORE_ADDED => {
                self.add_ignore_slot(guid, true, names);
                true
            }
            friend_result::IGNORE_REMOVED => {
                self.remove_ignore(guid, names);
                true
            }
            // Every other result shows its line alone.
            _ => return self.line(UiError::key(key), names),
        };
        let Some(name) = self.look_up(guid, Some(key), ignore, names) else {
            return;
        };
        if key == "ERR_FRIEND_ADDED_S" {
            self.set_name(guid, &name);
        }
        // The cached arm sorts the list the result acted on, pending or not, then shows the line
        // (`0x5acdde` → `0x5acde6`, `0x5ace00` → `0x5ace08`).
        if ignore {
            self.sort_ignores(names);
        } else {
            self.sort_friends(names);
        }
        self.line(named_line(key, &name), names);
    }

    /// The name cache's answer for `guid`, `found` or refused: each lookup that missed it runs its
    /// callback in the order made. The friends' (`0x5ace70`) stores a found name (`0x5acf0a`),
    /// drops the pending count (`0x5acf15`) and shows its line with the name (`0x5acf27`); for a
    /// refusal it shows `ERR_FRIEND_ERROR` (`0x5ace9f`), drops the count (`0x5aceb6`) and sends
    /// `CMSG_DEL_FRIEND` (`0x5acec8`). Neither line shows for the list's lookups (`0x1d1`). The
    /// ignores' (`0x5acf40`) drops its count and shows its line for a found name (`0x5acf97`,
    /// `0x5acfe2`); for a refusal it shows `ERR_FRIEND_ERROR` (`0x5acf6e`), sends
    /// `CMSG_DEL_IGNORE` (`0x5acf8c`) and drops its count (`0x5acf97`).
    fn apply_player_name(
        &mut self,
        guid: u64,
        found: Option<&str>,
        names: &dyn Names,
        send: &mut dyn FnMut(ClientCommand),
    ) {
        let (answered, waiting): (Vec<NameWait>, Vec<NameWait>) = std::mem::take(&mut self.waits)
            .into_iter()
            .partition(|w| w.guid == guid);
        self.waits = waiting;
        for wait in answered {
            match found {
                Some(name) => {
                    if !wait.ignore {
                        self.set_name(guid, name);
                    }
                    self.decrement_pending(wait.ignore, names);
                    if let Some(key) = wait.line {
                        self.line(named_line(key, name), names);
                    }
                }
                None => {
                    if wait.line.is_some() {
                        self.line(UiError::key("ERR_FRIEND_ERROR"), names);
                    }
                    if wait.ignore {
                        send(ClientCommand::DelIgnore { guid });
                        self.decrement_pending(true, names);
                    } else {
                        self.decrement_pending(false, names);
                        send(ClientCommand::DelFriend { guid });
                    }
                }
            }
        }
    }

    /// `GetSelectedFriendIndex` (`0x5ae510`) plus one, as `GetSelectedFriend` answers it
    /// (`0x5ad272`): the slot holding the selected guid, 0 for none.
    fn selected_friend_index(&self) -> u32 {
        row_of(self.slots.iter().map(|s| s.guid), self.selected_friend)
    }

    /// `GetSelectedIgnore` (`0x5ae630` plus one): the slot holding the selected guid, 0 for none.
    fn selected_ignore_index(&self) -> u32 {
        row_of(self.ignores.iter().copied(), self.selected_ignore)
    }

    /// `SMSG_WHO`: the answer to the last query.
    fn apply_who(&mut self, results: benilla_protocol::messages::WhoResults) {
        self.who = results.entries;
        self.who_total = results.total;
        self.who_dirty = true;
    }
}

/// `CompareFriends` (`0x5ae870`): an empty slot after a full one (`0x5ae876`-`0x5ae8b2`), an
/// offline slot after an online one (`0x5ae8b5`, `0x5ae8f4`), then the names by `SStrCmpI` with
/// no bound (`0x5ae8ce`), a missing name after a present one.
fn compare_friends(a: &FriendSlot, b: &FriendSlot) -> i32 {
    match (a.guid == 0, b.guid == 0) {
        (true, true) => return 0,
        (true, false) => return 1,
        (false, true) => return -1,
        (false, false) => {}
    }
    if a.online != b.online {
        return if a.online { -1 } else { 1 };
    }
    match (&a.name, &b.name) {
        (Some(x), Some(y)) => benilla_ui::crt::str_cmp_i(x, y),
        (None, None) => 0,
        (None, Some(_)) => 1,
        (Some(_), None) => -1,
    }
}

/// The 1-based slot holding the selected `guid`, 0 for none (`0x5ae510`, `0x5ae630`, plus one).
pub(super) fn row_of(slots: impl IntoIterator<Item = u64>, guid: u64) -> u32 {
    if guid == 0 {
        return 0;
    }
    slots
        .into_iter()
        .position(|g| g == guid)
        .map_or(0, |k| k as u32 + 1)
}

/// `CompareIgnoredByName` (`0x5ae940`): an empty slot after a full one (`0x5ae96f`, `0x5ae975`),
/// then the names the cache holds, read with no callback (`0x5ae98d`, `0x5ae9ab`), by `SStrCmpI`
/// with no bound (`0x5ae9c2`), a missing name after a present one.
fn compare_ignores(a: u64, b: u64, names: &dyn Names) -> i32 {
    match (a == 0, b == 0) {
        (true, true) => return 0,
        (true, false) => return 1,
        (false, true) => return -1,
        (false, false) => {}
    }
    match (names.read(a), names.read(b)) {
        (Some(x), Some(y)) => benilla_ui::crt::str_cmp_i(&x, &y),
        (None, None) => 0,
        (None, Some(_)) => 1,
        (Some(_), None) => -1,
    }
}

/// A result's line with the subject's name, which `DisplayError` is passed twice (`0x5acde3`); the
/// key's arity takes what it fills.
fn named_line(key: &'static str, name: &str) -> UiError {
    UiError::strings(key, &vec![name; name_pushes(key)])
}

/// The `GlobalStrings.lua` key each result shows, the catalog id its arm passes `DisplayError`
/// through the table at `0x5ace1c`: results 0 and 4 share `0x108`, 6 and 7 share `0x105`, and
/// every result past 17 shows `0x10b` (`0x5acab9` → `0x5acd85`). No result shows `0x103`,
/// `ERR_FRIEND_DB_ERROR`.
fn result_key(result: u8) -> &'static str {
    match result {
        friend_result::DB_ERROR | friend_result::NOT_FOUND => "ERR_FRIEND_NOT_FOUND",
        friend_result::LIST_FULL => "ERR_FRIEND_LIST_FULL",
        friend_result::ONLINE => "ERR_FRIEND_ONLINE_SS",
        friend_result::OFFLINE => "ERR_FRIEND_OFFLINE_S",
        friend_result::REMOVED => "ERR_FRIEND_REMOVED_S",
        friend_result::ADDED_ONLINE | friend_result::ADDED_OFFLINE => "ERR_FRIEND_ADDED_S",
        friend_result::ALREADY => "ERR_FRIEND_ALREADY_S",
        friend_result::SELF => "ERR_FRIEND_SELF",
        friend_result::ENEMY => "ERR_FRIEND_WRONG_FACTION",
        friend_result::IGNORE_FULL => "ERR_IGNORE_FULL",
        friend_result::IGNORE_SELF => "ERR_IGNORE_SELF",
        friend_result::IGNORE_NOT_FOUND => "ERR_IGNORE_NOT_FOUND",
        friend_result::IGNORE_ALREADY => "ERR_IGNORE_ALREADY_S",
        friend_result::IGNORE_ADDED => "ERR_IGNORE_ADDED_S",
        friend_result::IGNORE_REMOVED => "ERR_IGNORE_REMOVED_S",
        friend_result::IGNORE_AMBIGUOUS => "ERR_IGNORE_AMBIGUOUS",
        _ => "ERR_FRIEND_ERROR",
    }
}

/// How many times the subject's name fills a result's message, as the key's suffix names it: none
/// for a bare key, one for `_S`, two for `_SS` (the `|Hplayer:%s|h[%s]|h` link). Read off the key,
/// not the text, so a string not yet resolved never passes for one that takes no name.
fn name_pushes(key: &str) -> usize {
    if key.ends_with("_SS") {
        2
    } else if key.ends_with("_S") {
        1
    } else {
        0
    }
}

/// The social packet handlers: the friend and ignore lists, the result codes, the name answers
/// the lists waited on, and the `/who` answer. The lines queue by key for the action feed, and the
/// list events fire from the social feed.
pub(crate) mod net {
    use super::*;
    use benilla_protocol::{SessionEvent, SessionEventKind};

    use crate::names::NameCache;
    use crate::net::{NetCommands, NetHandlerApp};
    use crate::query_cache::Ask;

    /// Register the handlers, one per kind, and the teardown listeners.
    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::FriendList, on_friend_list)
            .net_handler(K::IgnoreList, on_ignore_list)
            .net_handler(K::FriendStatus, on_friend_status)
            .net_handler(K::PlayerName, on_player_name)
            .net_handler(K::WhoResults, on_who)
            .net_handler(K::LoggedOut, on_session_end)
            .net_handler(K::Disconnected, on_session_end);
    }

    /// The name cache as a handler holds it: the lookup with a callback asks the server on a
    /// miss ([`Ask::Event`]), the one with none never does ([`Ask::Read`]).
    pub(crate) struct CacheNames<'a> {
        pub(crate) names: &'a NameCache,
        pub(crate) net: &'a NetCommands,
    }

    impl Names for CacheNames<'_> {
        fn lookup(&self, guid: u64) -> Option<String> {
            self.names
                .resolve_player(guid, Ask::Event(self.net))
                .map(str::to_string)
        }
        fn read(&self, guid: u64) -> Option<String> {
            self.names
                .resolve_player(guid, Ask::Read)
                .map(str::to_string)
        }
    }

    fn on_friend_list(
        In(ev): In<SessionEvent>,
        mut social: ResMut<SocialState>,
        names: Res<NameCache>,
        net: Res<NetCommands>,
    ) {
        if let SessionEvent::FriendList { friends } = ev {
            let names = CacheNames {
                names: &names,
                net: &net,
            };
            friend_list(&mut social, friends, &names);
        }
    }

    fn on_ignore_list(
        In(ev): In<SessionEvent>,
        mut social: ResMut<SocialState>,
        names: Res<NameCache>,
        net: Res<NetCommands>,
    ) {
        if let SessionEvent::IgnoreList { guids } = ev {
            let names = CacheNames {
                names: &names,
                net: &net,
            };
            ignore_list(&mut social, guids, &names);
        }
    }

    fn on_friend_status(
        In(ev): In<SessionEvent>,
        mut social: ResMut<SocialState>,
        names: Res<NameCache>,
        net: Res<NetCommands>,
    ) {
        if let SessionEvent::FriendStatus(update) = ev {
            let names = CacheNames {
                names: &names,
                net: &net,
            };
            social.apply_friend_status(update, &names);
        }
    }

    /// `SMSG_NAME_QUERY_RESPONSE`, found or refused: the lookups that waited on it run their
    /// callbacks. The name cache's handler, registered ahead of every other, has filed it.
    fn on_player_name(
        In(ev): In<SessionEvent>,
        mut social: ResMut<SocialState>,
        names: Res<NameCache>,
        net: Res<NetCommands>,
    ) {
        if let SessionEvent::PlayerName { guid, name, .. } = ev {
            let found = (!name.is_empty()).then_some(name.as_str());
            let cache = CacheNames {
                names: &names,
                net: &net,
            };
            social.apply_player_name(guid, found, &cache, &mut |c| {
                let _ = net.0.send(c);
            });
        }
    }

    fn on_who(In(ev): In<SessionEvent>, mut social: ResMut<SocialState>) {
        if let SessionEvent::WhoResults(results) = ev {
            who(&mut social, results);
        }
    }

    /// The world's teardown `ClientDestroyGame` (`0x401ee0`) frees the list (`0x5ae2d0` at
    /// `0x401fc6`), at a logout to the character list and a lost link alike. `clear_session`,
    /// not `default()`, so the per-process sort chain survives.
    fn on_session_end(In(ev): In<SessionEvent>, mut social: ResMut<SocialState>) {
        if let SessionEvent::LoggedOut | SessionEvent::Disconnected { .. } = ev {
            social.clear_session();
        }
    }

    /// `SMSG_FRIEND_LIST`.
    pub(crate) fn friend_list(
        social: &mut SocialState,
        friends: Vec<FriendEntry>,
        names: &dyn Names,
    ) {
        social.apply_friend_list(&friends, names);
    }

    /// `SMSG_IGNORE_LIST`.
    pub(crate) fn ignore_list(social: &mut SocialState, guids: Vec<u64>, names: &dyn Names) {
        social.apply_ignore_list(&guids, names);
    }

    /// `SMSG_WHO`.
    pub(crate) fn who(social: &mut SocialState, results: benilla_protocol::messages::WhoResults) {
        social.apply_who(results);
    }
}

/// The social window's session: the wire mirror, the VM feed, and the outbound intents.
pub(crate) struct UiSocialPlugin;

impl Plugin for UiSocialPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        // Category 0 (`0x5adcfd`).
        app.console_command(
            "whois",
            crate::console::DEBUG,
            Some(whois::HELP),
            whois::whois,
        );
        app.init_resource::<SocialState>()
            .add_systems(
                Startup,
                load_lfg_dungeons.after(benilla_assets::AssetSet::Open),
            )
            .add_systems(
                Update,
                (
                    feed::feed_social.in_set(UiFeed),
                    feed::drain_social.in_set(crate::ui_script::UiDrain),
                ),
            );
    }
}

/// `LFGDungeons.dbc` for the VM's looking-for-group lists.
#[derive(Resource)]
struct LfgDungeonsRes(Vec<benilla_ui::script::LfgDungeon>);

fn load_lfg_dungeons(mut commands: Commands, assets: Option<Res<benilla_assets::WorldAssets>>) {
    use benilla_assets::LockRecover;
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_lfg_dungeons(&mut chain)
    };
    match loaded {
        Ok(rows) => commands.insert_resource(LfgDungeonsRes(
            rows.into_iter()
                .map(|r| benilla_ui::script::LfgDungeon {
                    name: r.name,
                    min_level: r.min_level,
                    max_level: r.max_level,
                    kind: r.kind,
                    side: r.side,
                })
                .collect(),
        )),
        Err(e) => {
            warn!("ui_social: LFGDungeons.dbc failed to load — the LFG lists stay empty: {e:#}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::FriendOnline;

    fn status(result: u8, guid: u64) -> FriendStatusUpdate {
        FriendStatusUpdate {
            result,
            guid,
            online: None,
        }
    }

    /// Every name cached, as `0x55f080` returns them.
    struct Cached;

    impl Names for Cached {
        fn lookup(&self, guid: u64) -> Option<String> {
            self.read(guid)
        }
        fn read(&self, guid: u64) -> Option<String> {
            (guid != 0).then(|| format!("P{guid}"))
        }
    }

    fn guids(social: &SocialState) -> Vec<u64> {
        social
            .slots
            .iter()
            .map(|s| s.guid)
            .take_while(|&g| g != 0)
            .collect()
    }

    /// An add claims a slot once (`0x5aea60`), a remove clears its guid and name and nothing else
    /// (`0x5aea00`), and the sort moves the hole last (`0x5ae876`).
    #[test]
    fn add_and_remove_results_maintain_the_slots() {
        let mut social = SocialState::default();
        social.apply_friend_status(
            FriendStatusUpdate {
                result: friend_result::ADDED_ONLINE,
                guid: 7,
                online: Some(FriendOnline {
                    status: friend_status::ONLINE,
                    area: 12,
                    level: 60,
                    class: 4,
                }),
            },
            &Cached,
        );
        social.apply_friend_status(status(friend_result::ADDED_OFFLINE, 8), &Cached);
        assert_eq!(guids(&social), [7, 8]);
        assert_eq!(social.slots[0].level, 60);
        assert_eq!(
            social.slots[0].name.as_deref(),
            Some("P7"),
            "an add stores the name"
        );

        social.apply_friend_status(status(friend_result::ADDED_OFFLINE, 7), &Cached);
        assert_eq!(guids(&social), [7, 8], "the same add again reuses the slot");
        assert!(!social.slots[0].online, "now offline");

        social.apply_friend_status(status(friend_result::REMOVED, 8), &Cached);
        assert_eq!(guids(&social), [7]);
        assert!(social.slots[49].name.is_none());
        let events = std::mem::take(&mut social.steps)
            .into_iter()
            .filter(|s| matches!(s.heard, Heard::Event(_)))
            .count();
        assert_eq!(events, 4, "each cached result sorts and signals");
    }

    /// The selection is a guid no remove clears (`0x5aea00` writes only the slot); it reads as
    /// none while no slot holds it (`0x5ae546`).
    #[test]
    fn removing_the_selected_friend_leaves_its_guid_selected() {
        let mut social = SocialState::default();
        social.apply_friend_list(
            &[FriendEntry {
                guid: 7,
                ..Default::default()
            }],
            &Cached,
        );
        social.selected_friend = 7;
        assert_eq!(social.selected_friend_index(), 1);
        social.apply_friend_status(status(friend_result::REMOVED, 7), &Cached);
        assert_eq!(social.selected_friend, 7);
        assert_eq!(social.selected_friend_index(), 0);
    }

    /// A list shorter than the slots leaves the slots past it as they were, unnamed
    /// (`0x5ae366`-`0x5ae388`, the loop to the count at `0x5ae46f`).
    #[test]
    fn a_shorter_list_keeps_the_slots_past_it() {
        let mut social = SocialState::default();
        let friend = |guid| FriendEntry {
            guid,
            ..Default::default()
        };
        social.apply_friend_list(&[friend(7), friend(8)], &Cached);
        social.apply_friend_list(&[friend(9)], &NoNames);
        assert_eq!(guids(&social), [9, 8]);
        assert!(social.slots.iter().all(|s| s.name.is_none()));
        assert_eq!(social.friends_pending, 1);
    }

    #[test]
    fn ignore_results_maintain_the_ignore_list() {
        let mut social = SocialState::default();
        assert!(!social.is_ignored(9));
        social.apply_friend_status(status(friend_result::IGNORE_ADDED, 9), &Cached);
        assert!(social.is_ignored(9));
        assert!(!social.is_ignored(0), "guid 0 is never ignored");
        social.apply_friend_status(status(friend_result::IGNORE_REMOVED, 9), &Cached);
        assert!(!social.is_ignored(9));
    }

    /// The table at `0x5ace1c`: 18 results and one key for everything past them.
    #[test]
    fn every_result_code_maps_to_its_line() {
        assert_eq!(result_key(friend_result::DB_ERROR), "ERR_FRIEND_NOT_FOUND");
        assert_eq!(
            result_key(friend_result::ADDED_ONLINE),
            result_key(friend_result::ADDED_OFFLINE),
        );
        for result in 0x12..=0xffu8 {
            assert_eq!(result_key(result), "ERR_FRIEND_ERROR", "{result:#04x}");
        }
        assert_eq!(
            result_key(friend_result::IGNORE_NOT_FOUND),
            "ERR_IGNORE_NOT_FOUND"
        );
    }

    /// The comparator's three keys in their order (`0x5ae870`).
    #[test]
    fn the_comparator_puts_empty_then_offline_then_unnamed_last() {
        let slot = |guid, online, name: Option<&str>| FriendSlot {
            guid,
            online,
            name: name.map(str::to_string),
            ..Default::default()
        };
        let empty = slot(0, true, Some("a"));
        let off = slot(1, false, Some("a"));
        let on_b = slot(2, true, Some("b"));
        let on_a = slot(3, true, Some("A"));
        let on_none = slot(4, true, None);
        assert_eq!(compare_friends(&empty, &off), 1);
        assert_eq!(compare_friends(&empty, &empty), 0);
        assert_eq!(compare_friends(&off, &on_none), 1);
        assert_eq!(compare_friends(&on_b, &on_a), 1);
        assert_eq!(
            compare_friends(&on_a, &slot(5, true, Some("a"))),
            0,
            "without case"
        );
        assert_eq!(compare_friends(&on_none, &on_a), 1);
        assert_eq!(compare_friends(&on_none, &on_none), 0);
    }

    #[test]
    fn a_logout_keeps_the_sort_chain_and_drops_everything_else() {
        let mut social = SocialState::default();
        social.apply_friend_list(
            &[FriendEntry {
                guid: 7,
                ..Default::default()
            }],
            &Cached,
        );
        social.apply_ignore_list(&[9], &Cached);
        social.who_to_ui = true;
        social.who_sort.promote("level");
        social.who_sort.promote("level"); // descending
        let chain = social.who_sort.clone();

        social.clear_session();
        assert!(guids(&social).is_empty());
        assert!(!social.is_ignored(9));
        assert!(social.who.is_empty());
        assert!(!social.who_to_ui, "the frame is closed after a logout");
        assert_eq!(social.who_sort, chain, "the sort chain is process state");
        assert_ne!(
            social.who_sort,
            benilla_ui::script::WhoSortChain::default(),
            "and the assertion above only means something if it is not the seeded chain"
        );
    }

    #[test]
    fn the_key_suffix_is_the_name_arity() {
        assert_eq!(name_pushes("ERR_FRIEND_ONLINE_SS"), 2);
        assert_eq!(name_pushes("ERR_FRIEND_ADDED_S"), 1);
        assert_eq!(name_pushes("ERR_FRIEND_SELF"), 0, "not an _S despite the S");
    }

    #[test]
    fn every_key_resolves_and_its_arity_matches_the_string() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let s = benilla_ui::script::UiScript::new().expect("VM");
        s.run(&String::from_utf8_lossy(&src)).expect("runs clean");

        let mut keys: Vec<&str> = (0x00..=0x12u8).map(result_key).collect();
        // The away tags `GetFriendInfo` reads (`0x849acc`, `0x849adc`).
        keys.extend(["CHAT_FLAG_AFK", "CHAT_FLAG_DND"]);
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), 19, "17 result rows plus the two away tags");

        for key in keys {
            let text: String = s.lua().globals().get(key).expect(key);
            assert!(!text.is_empty(), "{key} resolves empty");
            // The away tags are a row field, not a `DisplayError` message: no catalog row.
            if !key.starts_with("CHAT_FLAG_") {
                assert!(
                    benilla_ui::messages::by_key(key).is_some(),
                    "{key} is not a catalog row, so its surface and sound would be a guess"
                );
                assert_eq!(
                    text.matches("%s").count(),
                    name_pushes(key),
                    "{key}: the string's holes vs the arity its suffix promises"
                );
            }
        }
        assert_eq!(
            benilla_ui::messages::by_key("ERR_FRIEND_ONLINE_SS").and_then(|r| r.sound),
            Some("FRIENDJOINGAME"),
            "the cue the chat-log path was dropping"
        );
    }
}
