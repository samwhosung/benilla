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
    ignores: Vec<u64>,
    /// The selected ignore as a guid (the reference's `+0x720`).
    selected_ignore: u64,
    /// The ignore guids in the order the feed last showed them, for Lua's row indices.
    ignore_display_order: Vec<u64>,
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
    /// The ignored players whose name `SMSG_IGNORE_LIST` looked up since the last feed
    /// (`0x5af59a`), which the feed asks with a callback.
    name_events: std::collections::HashSet<u64>,
    /// The `FRIENDLIST_UPDATE`s the sorts signalled (`0x5ae867`) since the last feed.
    friend_signals: u32,
    ignores_dirty: bool,
    who_dirty: bool,
}

impl Default for SocialState {
    fn default() -> Self {
        Self {
            slots: vec![FriendSlot::default(); FRIEND_SLOTS],
            friends_pending: 0,
            selected_friend: 0,
            ignores: Vec::new(),
            selected_ignore: 0,
            ignore_display_order: Vec::new(),
            who: Vec::new(),
            who_total: 0,
            who_sort: Default::default(),
            who_to_ui: false,
            waits: Vec::new(),
            name_events: Default::default(),
            friend_signals: 0,
            ignores_dirty: false,
            who_dirty: false,
        }
    }
}

/// The lookup with a callback a friend-list function makes (`0x55f080`): the cached name, or
/// `None` after asking the server.
type Lookup<'a> = &'a dyn Fn(u64) -> Option<String>;

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

    /// Is `guid` on the ignore list? The reference's `IsIgnored` (`0x5ae5a0`): an ignored player's
    /// chat and text emotes drop silently (`0x49dbe0`), and their duel is declined (`0x4d4a33`).
    pub(crate) fn is_ignored(&self, guid: u64) -> bool {
        guid != 0 && self.ignores.contains(&guid)
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
    fn sort_friends(&mut self) {
        benilla_ui::crt::qsort(&mut self.slots, compare_friends);
        self.friend_signals += 1;
    }

    /// `DecrementPendingFriendNames` (`0x5aeb70`): a pending count above 0 drops by one, and the
    /// list sorts when it reaches 0.
    fn decrement_friends_pending(&mut self) {
        if self.friends_pending == 0 {
            return;
        }
        self.friends_pending -= 1;
        if self.friends_pending == 0 {
            self.sort_friends();
        }
    }

    /// Look `guid`'s name up for a friend function, the miss counted and its callback waited for.
    fn look_up(&mut self, guid: u64, line: Option<&'static str>, lookup: Lookup) -> Option<String> {
        let name = lookup(guid);
        if name.is_none() {
            self.friends_pending = self.friends_pending.wrapping_add(1);
            self.waits.push(NameWait {
                guid,
                line,
                ignore: false,
            });
        }
        name
    }

    /// `SMSG_FRIEND_LIST` (`0x5ae350`): every slot's name freed (`0x5ae366`-`0x5ae388`), then up to
    /// 50 records into the slots from the first, a status of 0 storing zeros for the area, level
    /// and class (`0x5ae415`); the slots past the records keep their guids. Each name is copied
    /// from the cache (`0x5ae438`-`0x5ae451`) or counted pending (`0x5ae459`), and the list sorts
    /// only with nothing pending (`0x5ae475`-`0x5ae481`).
    fn apply_friend_list(&mut self, friends: &[FriendEntry], lookup: Lookup) {
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
            let name = self.look_up(entry.guid, None, lookup);
            self.slots[k].name = name;
        }
        if self.friends_pending == 0 {
            self.sort_friends();
        }
    }

    /// Seat the ignore list directly, for tests of the code that consults it.
    #[cfg(test)]
    pub(crate) fn set_ignores_for_test(&mut self, guids: Vec<u64>) {
        self.apply_ignore_list(guids);
    }

    /// `SMSG_IGNORE_LIST`: the whole list.
    fn apply_ignore_list(&mut self, guids: Vec<u64>) {
        self.name_events.extend(guids.iter().copied());
        self.ignores = guids;
        self.ignores_dirty = true;
    }

    /// `SMSG_FRIEND_STATUS`, `HandleResult` (`0x5acab0`) through its table (`0x5ace1c`): a result
    /// that names no one shows its line alone; the others act on a list, then look the name up
    /// with their line as the callback's message. A cached name is stored for an add
    /// (`0x5acdbd`), the list sorts and the line shows with the name (`0x5acdd5`-`0x5ace08`); a
    /// miss counts as pending and returns unsignalled (`0x5acba6`-`0x5acdba`).
    fn apply_friend_status(&mut self, update: FriendStatusUpdate, lookup: Lookup) -> Vec<UiError> {
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
                if !self.ignores.contains(&guid) {
                    self.ignores.push(guid);
                }
                self.ignores_dirty = true;
                true
            }
            friend_result::IGNORE_REMOVED => {
                self.ignores.retain(|g| *g != guid);
                if self.selected_ignore == guid {
                    self.selected_ignore = 0;
                }
                self.ignores_dirty = true;
                true
            }
            // Every other result shows its line alone.
            _ => return vec![UiError::key(key)],
        };
        let name = if ignore {
            let name = lookup(guid);
            if name.is_none() {
                self.waits.push(NameWait {
                    guid,
                    line: Some(key),
                    ignore,
                });
            }
            name
        } else {
            let name = self.look_up(guid, Some(key), lookup);
            if let Some(name) = &name {
                if key == "ERR_FRIEND_ADDED_S" {
                    self.set_name(guid, name);
                }
                self.sort_friends();
            }
            name
        };
        name.map(|name| named_line(key, &name))
            .into_iter()
            .collect()
    }

    /// The name cache's answer for `guid`, `found` or refused: each lookup that missed it runs its
    /// callback in the order made. The friends' (`0x5ace70`) stores a found name (`0x5acf0a`),
    /// drops the pending count (`0x5acf15`) and shows its line with the name (`0x5acf27`); for a
    /// refusal it shows `ERR_FRIEND_ERROR` (`0x5ace9f`), drops the count (`0x5aceb6`) and sends
    /// `CMSG_DEL_FRIEND` (`0x5acec8`). Neither line shows for the list's lookups (`0x1d1`). The
    /// ignores' callback (`0x5acf40`) shows the same lines.
    fn apply_player_name(
        &mut self,
        guid: u64,
        found: Option<&str>,
        send: &mut dyn FnMut(ClientCommand),
    ) -> Vec<UiError> {
        let mut lines = Vec::new();
        let (answered, waiting): (Vec<NameWait>, Vec<NameWait>) = std::mem::take(&mut self.waits)
            .into_iter()
            .partition(|w| w.guid == guid);
        self.waits = waiting;
        for wait in answered {
            match found {
                Some(name) => {
                    if !wait.ignore {
                        self.set_name(guid, name);
                        self.decrement_friends_pending();
                    }
                    lines.extend(wait.line.map(|key| named_line(key, name)));
                }
                None => {
                    lines.extend(wait.line.map(|_| UiError::key("ERR_FRIEND_ERROR")));
                    if !wait.ignore {
                        self.decrement_friends_pending();
                        send(ClientCommand::DelFriend { guid });
                    }
                }
            }
        }
        lines
    }

    /// `GetSelectedFriendIndex` (`0x5ae510`) plus one, as `GetSelectedFriend` answers it
    /// (`0x5ad272`): the slot holding the selected guid, 0 for none.
    fn selected_friend_index(&self) -> u32 {
        if self.selected_friend == 0 {
            return 0;
        }
        self.find(self.selected_friend).map_or(0, |k| k as u32 + 1)
    }

    /// `GetSelectedIgnore`'s answer: the shown row holding the selected guid, 0 for none.
    fn selected_ignore_index(&self) -> u32 {
        index_of(&self.ignore_display_order, self.selected_ignore)
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

/// The 1-based row of a guid in the shown order, 0 when absent.
fn index_of(order: &[u64], guid: u64) -> u32 {
    if guid == 0 {
        return 0;
    }
    order
        .iter()
        .position(|g| *g == guid)
        .map_or(0, |i| i as u32 + 1)
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
    use crate::ui_action::UiErrorKeys;

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

    /// The lookup with a callback (`0x55f080`), which asks the server on a miss.
    fn lookup<'a>(
        names: &'a NameCache,
        commands: &'a NetCommands,
    ) -> impl Fn(u64) -> Option<String> + 'a {
        move |guid| {
            names
                .resolve_player(guid, Ask::Event(commands))
                .map(str::to_string)
        }
    }

    fn on_friend_list(
        In(ev): In<SessionEvent>,
        mut social: ResMut<SocialState>,
        names: Res<NameCache>,
        commands: Res<NetCommands>,
    ) {
        if let SessionEvent::FriendList { friends } = ev {
            friend_list(&mut social, friends, &lookup(&names, &commands));
        }
    }

    fn on_ignore_list(In(ev): In<SessionEvent>, mut social: ResMut<SocialState>) {
        if let SessionEvent::IgnoreList { guids } = ev {
            ignore_list(&mut social, guids);
        }
    }

    fn on_friend_status(
        In(ev): In<SessionEvent>,
        mut social: ResMut<SocialState>,
        names: Res<NameCache>,
        commands: Res<NetCommands>,
        errors: Res<UiErrorKeys>,
    ) {
        if let SessionEvent::FriendStatus(update) = ev {
            let lines = social.apply_friend_status(update, &lookup(&names, &commands));
            errors.extend(lines);
        }
    }

    /// `SMSG_NAME_QUERY_RESPONSE`, found or refused: the lookups that waited on it run their
    /// callbacks. The name cache's handler, registered ahead of every other, has filed it.
    fn on_player_name(
        In(ev): In<SessionEvent>,
        mut social: ResMut<SocialState>,
        commands: Res<NetCommands>,
        errors: Res<UiErrorKeys>,
    ) {
        if let SessionEvent::PlayerName { guid, name, .. } = ev {
            let found = (!name.is_empty()).then_some(name.as_str());
            let lines = social.apply_player_name(guid, found, &mut |c| {
                let _ = commands.0.send(c);
            });
            errors.extend(lines);
        }
    }

    fn on_who(In(ev): In<SessionEvent>, mut social: ResMut<SocialState>) {
        if let SessionEvent::WhoResults(results) = ev {
            who(&mut social, results);
        }
    }

    /// The world's teardown `ClientDestroyGame` (`0x401ee0`) frees the list (`0x5ae2d0` at
    /// `0x401fc6`): a logout to the character list and a lost link alike. `clear_session`, not `default()`, so the per-process
    /// sort chain survives.
    fn on_session_end(In(ev): In<SessionEvent>, mut social: ResMut<SocialState>) {
        if let SessionEvent::LoggedOut | SessionEvent::Disconnected { .. } = ev {
            social.clear_session();
        }
    }

    /// `SMSG_FRIEND_LIST`, each name looked up with `lookup`.
    pub(crate) fn friend_list(
        social: &mut SocialState,
        friends: Vec<FriendEntry>,
        lookup: &dyn Fn(u64) -> Option<String>,
    ) {
        social.apply_friend_list(&friends, lookup);
    }

    /// `SMSG_IGNORE_LIST`.
    pub(crate) fn ignore_list(social: &mut SocialState, guids: Vec<u64>) {
        social.apply_ignore_list(guids);
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
    fn cached(guid: u64) -> Option<String> {
        Some(format!("P{guid}"))
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
        let _ = social.apply_friend_status(
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
            &cached,
        );
        let _ = social.apply_friend_status(status(friend_result::ADDED_OFFLINE, 8), &cached);
        assert_eq!(guids(&social), [7, 8]);
        assert_eq!(social.slots[0].level, 60);
        assert_eq!(
            social.slots[0].name.as_deref(),
            Some("P7"),
            "an add stores the name"
        );

        let _ = social.apply_friend_status(status(friend_result::ADDED_OFFLINE, 7), &cached);
        assert_eq!(guids(&social), [7, 8], "the same add again reuses the slot");
        assert!(!social.slots[0].online, "now offline");

        let _ = social.apply_friend_status(status(friend_result::REMOVED, 8), &cached);
        assert_eq!(guids(&social), [7]);
        assert!(social.slots[49].name.is_none());
        assert_eq!(
            social.friend_signals, 4,
            "each cached result sorts and signals"
        );
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
            &cached,
        );
        social.selected_friend = 7;
        assert_eq!(social.selected_friend_index(), 1);
        let _ = social.apply_friend_status(status(friend_result::REMOVED, 7), &cached);
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
        social.apply_friend_list(&[friend(7), friend(8)], &cached);
        social.apply_friend_list(&[friend(9)], &|_| None);
        assert_eq!(guids(&social), [9, 8]);
        assert!(social.slots.iter().all(|s| s.name.is_none()));
        assert_eq!(social.friends_pending, 1);
    }

    #[test]
    fn ignore_results_maintain_the_ignore_list() {
        let mut social = SocialState::default();
        assert!(!social.is_ignored(9));
        let _ = social.apply_friend_status(status(friend_result::IGNORE_ADDED, 9), &cached);
        assert!(social.is_ignored(9));
        assert!(!social.is_ignored(0), "guid 0 is never ignored");
        let _ = social.apply_friend_status(status(friend_result::IGNORE_REMOVED, 9), &cached);
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
            &cached,
        );
        social.apply_ignore_list(vec![9]);
        social.who_to_ui = true;
        social.who_sort.promote("level");
        social.who_sort.promote("level"); // descending
        let chain = social.who_sort.clone();

        social.clear_session();
        assert!(guids(&social).is_empty());
        assert!(social.ignores.is_empty());
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
