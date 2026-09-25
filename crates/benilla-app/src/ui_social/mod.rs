//! The social session — the friend list, the ignore list, `/who`, and the system lines they
//! print.
//!
//! [`SocialState`] mirrors the wire the way [`crate::ui_party`]'s `GroupState` does: the three
//! server packets replace it wholesale or patch one row, and the feed turns it into the VM
//! snapshot the FriendsFrame reads. Two client-side laws of the reference's `FriendList.cpp` (the
//! 0x728-byte object at `DAT_00c28168`) shape everything here:
//!
//! - **The lists are guids.** A friend slot holds `{status, note*, guid, area, level, class}` —
//!   no name — and the display name comes from the ObjectMgr name cache (`0x55f080`) at format
//!   time. So does ours ([`crate::names::NameCache`]), which is why a freshly-listed friend can
//!   take a name-query round trip to show a name, and why removal (a guid on the wire) has to
//!   look the name up first.
//! - **Selection is a guid too** (`+0x648` friend, `+0x720` ignore), converted to an index on
//!   read. Ours is stored the same way, so a list that re-sorts under a selection keeps the same
//!   player selected rather than the same row.
//!
//! ## The system lines
//!
//! Every friend/ignore result prints one line, and they go to the **chat frame**, not the red
//! error line. That is byte-settled rather than assumed: `FriendList::HandleResult 0x5acab0`
//! displays through `CGGameUI::DisplayError 0x496720` with catalog ids `0x104..0x114`, and each
//! of those rows' `+0x4` **kind** field is written from the register the independently-VERIFIED
//! kind-0 row `0x8b` (`ERR_QUEST_FAILED_S` → chat) also uses — `ecx`, i.e. **kind 0 = the chat
//! composer** (`0x49a870`). Read at the bytes in the table's static initializer,
//! `0x486f78`–`0x48719c` (base `0xb4b498`, stride `0x14`); record `0x115` is the first to take a
//! different register, which is exactly where the friend band ends.
//!
//! Which result maps to which GlobalStrings key is *inferred by name* (the `ERR_FRIEND_*` /
//! `ERR_IGNORE_*` set is one-to-one with vmangos's `FriendsResult` enum, and 18 results collapse
//! to the 17 ids because both ADDED codes share `ERR_FRIEND_ADDED_S`) — still open.
//!
//! ## What the ignore list is *for*
//!
//! Not the window — the silence. `FriendList::IsIgnored 0x5ae5a0` gates inbound chat and text
//! emotes (the `SMSG_TEXT_EMOTE` handler `0x49dbe0`: ignored performer ⇒ dropped
//! silently, no line at all) and the duel handler's auto-decline (`0x4d4a33`). [`is_ignored`] is
//! that predicate; its callers are the chat apply arm and the duel one, which is how decision
//! 0633's stated "no ignore list yet" deviation closes.

use benilla_protocol::messages::{friend_result, friend_status, FriendEntry, FriendStatusUpdate};
use bevy::prelude::*;

use crate::ui_script::{UiFeed, UiInput};

mod feed;
mod query;

pub(crate) use query::parse as who_query;

/// The social session mirror. Filled by the net drain's social arms, read by the feed, cleared on
/// disconnect beside the other per-login resources.
#[derive(Resource, Default)]
pub(crate) struct SocialState {
    /// The friend list as the wire sent it (`SMSG_FRIEND_LIST` order — vmangos walks a
    /// guid-keyed map). Display order is the feed's ([`Self::display_order`]).
    friends: Vec<FriendEntry>,
    /// The ignore list: guids, nothing else.
    ignores: Vec<u64>,
    /// The selected friend, stored as a **guid** like the reference's `+0x648`; `0` = none. A
    /// re-sort or a roster change keeps the same *player* selected.
    selected_friend: u64,
    /// The selected ignore (the reference's `+0x720`).
    selected_ignore: u64,
    /// The friend guids in the order the feed last showed them — written by the feed, read by the
    /// drain so a row index from Lua maps back to the same player the user clicked.
    display_order: Vec<u64>,
    /// The ignore guids in shown order, same contract.
    ignore_display_order: Vec<u64>,
    /// The last `/who` answer, and the server's true match total.
    who: Vec<benilla_protocol::messages::WhoEntry>,
    who_total: u32,
    /// The `/who` sort chain (`SortWho`'s seven `{key, dir}` slots at `0xc2817c`) — the app's
    /// authoritative copy, pushed to the VM with the rows so the binding can promote and re-sort
    /// synchronously. **Survives a logout** ([`SocialState::clear_session`]).
    who_sort: benilla_ui::script::WhoSortChain,
    /// `SetWhoToUI` — does the *next* `/who` answer belong to the Who frame (true) or the chat
    /// frame (false)? The WhoFrame's OnShow/OnHide drive it; a `/who` typed with the frame closed
    /// prints its results as chat lines.
    who_to_ui: bool,
    /// Results whose line is still owed, waiting on a name query (the reference's own
    /// resolve-then-compose order — `FriendList` formats from the name cache).
    pending_lines: Vec<FriendStatusUpdate>,
    /// Set whenever a list changed, so the feed knows to fire the Era update event.
    friends_dirty: bool,
    /// An explicit `ShowFriends()` is out: the next list answers it with `FRIENDLIST_SHOW`
    /// rather than `FRIENDLIST_UPDATE` (the reference's two arms, 1959).
    pub(crate) friends_show_pending: bool,
    ignores_dirty: bool,
    who_dirty: bool,
}

impl SocialState {
    /// Drop everything the socket owned, keeping what the *process* owns.
    ///
    /// Only [`Self::who_sort`] survives, and it survives because the reference's chain is
    /// per-process: its initialiser `0x5adc50` is reached once from the process-start run at
    /// `0x401666`, never from a login, so a player who left the who list sorted by level
    /// descending finds it that way after a relog. Everything else is login-scoped for the reasons
    /// decision 0668 gives — the server re-pushes both lists at the next login, and a stale ignore
    /// list would silence the wrong guids.
    pub(crate) fn clear_session(&mut self) {
        *self = Self {
            who_sort: std::mem::take(&mut self.who_sort),
            ..Self::default()
        };
    }

    /// Is `guid` on the ignore list? The reference's `FriendList::IsIgnored 0x5ae5a0` — the
    /// predicate inbound chat, text emotes and duel challenges all gate on (module doc).
    pub(crate) fn is_ignored(&self, guid: u64) -> bool {
        guid != 0 && self.ignores.contains(&guid)
    }

    /// Is `guid` on the FRIEND list? The reference's `FriendList::FindFriendSlot 0x5ae810` —
    /// base `this+8`, stride `0x20`, bound `0x32`, the same triple `GetNumFriends 0x5ae490`
    /// counts over.
    ///
    /// Its one consumer is the guild sign-on/sign-off line's fourth conjunct, and its purpose is
    /// **de-duplication, not suppression**: `SMSG_FRIEND_STATUS` emits the same two chat ids with
    /// no CVar gate of its own, so a guildmate who is also a friend would otherwise be announced
    /// twice. [`crate::ui_guild`]'s line path is where that matters.
    pub(crate) fn is_friend(&self, guid: u64) -> bool {
        guid != 0 && self.friends.iter().any(|f| f.guid == guid)
    }

    /// `SMSG_FRIEND_LIST` — replace the list wholesale (it is never a delta).
    fn apply_friend_list(&mut self, friends: Vec<FriendEntry>) {
        self.friends = friends;
        self.friends_dirty = true;
    }

    /// Seat the ignore list directly — test-only, for the neighbours that consult it rather than
    /// build it (decision 1764's trade-request ladder, whose leg 2 answers on its own opcode).
    #[cfg(test)]
    pub(crate) fn set_ignores_for_test(&mut self, guids: Vec<u64>) {
        self.apply_ignore_list(guids);
    }

    /// `SMSG_IGNORE_LIST` — likewise.
    fn apply_ignore_list(&mut self, guids: Vec<u64>) {
        self.ignores = guids;
        self.ignores_dirty = true;
    }

    /// `SMSG_FRIEND_STATUS` — apply the result to the list, and queue its system line.
    ///
    /// The server sends **no** fresh list after an add/remove, so the local list has to follow
    /// the result codes; a presence broadcast (ONLINE/OFFLINE) patches the row in place, which is
    /// what keeps a friend's zone current without a refresh.
    fn apply_friend_status(&mut self, update: FriendStatusUpdate) {
        match update.result {
            friend_result::ADDED_ONLINE | friend_result::ADDED_OFFLINE => {
                if !self.friends.iter().any(|f| f.guid == update.guid) {
                    let online = update.online;
                    self.friends.push(FriendEntry {
                        guid: update.guid,
                        status: online.map_or(friend_status::OFFLINE, |o| o.status),
                        area: online.map_or(0, |o| o.area),
                        level: online.map_or(0, |o| o.level),
                        class: online.map_or(0, |o| o.class),
                    });
                }
                self.friends_dirty = true;
            }
            friend_result::REMOVED => {
                self.friends.retain(|f| f.guid != update.guid);
                if self.selected_friend == update.guid {
                    self.selected_friend = 0;
                }
                self.friends_dirty = true;
            }
            friend_result::ONLINE => {
                if let (Some(entry), Some(online)) = (self.friend_mut(update.guid), update.online) {
                    entry.status = online.status;
                    entry.area = online.area;
                    entry.level = online.level;
                    entry.class = online.class;
                }
                self.friends_dirty = true;
            }
            friend_result::OFFLINE => {
                if let Some(entry) = self.friend_mut(update.guid) {
                    entry.status = friend_status::OFFLINE;
                    // The wire's own rule: an offline friend carries no area/level/class, so
                    // stale ones must not linger under the "Offline" row.
                    entry.area = 0;
                    entry.level = 0;
                    entry.class = 0;
                }
                self.friends_dirty = true;
            }
            friend_result::IGNORE_ADDED => {
                if !self.ignores.contains(&update.guid) {
                    self.ignores.push(update.guid);
                }
                self.ignores_dirty = true;
            }
            friend_result::IGNORE_REMOVED => {
                self.ignores.retain(|g| *g != update.guid);
                if self.selected_ignore == update.guid {
                    self.selected_ignore = 0;
                }
                self.ignores_dirty = true;
            }
            // Every other code is a refusal: it says something, and changes nothing.
            _ => {}
        }
        self.pending_lines.push(update);
    }

    fn friend_mut(&mut self, guid: u64) -> Option<&mut FriendEntry> {
        self.friends.iter_mut().find(|f| f.guid == guid)
    }

    /// `SMSG_WHO` — the answer to our last query.
    fn apply_who(&mut self, results: benilla_protocol::messages::WhoResults) {
        self.who = results.entries;
        self.who_total = results.total;
        self.who_dirty = true;
    }
}

/// The **message key** one friend/ignore result prints — resolved at the feed against the player's
/// own `GlobalStrings.lua`, never composed here.
///
/// The keys are what makes this table readable at all: two of its rows,
/// `ERR_FRIEND_NOT_FOUND` and `ERR_IGNORE_NOT_FOUND`, are the *same sentence* in enUS ("Player not
/// found.") and different strings everywhere else. A table of English cannot tell them apart, and
/// nothing that compared displayed text — a test of ours included — ever could.
///
/// Composed engine-side in the reference too: the FrameXML never names these
/// keys, so there is no Lua path that would resolve them for us.
fn result_key(result: u8) -> Option<&'static str> {
    Some(match result {
        friend_result::DB_ERROR => "ERR_FRIEND_DB_ERROR",
        friend_result::LIST_FULL => "ERR_FRIEND_LIST_FULL",
        friend_result::ONLINE => "ERR_FRIEND_ONLINE_SS",
        friend_result::OFFLINE => "ERR_FRIEND_OFFLINE_S",
        friend_result::NOT_FOUND => "ERR_FRIEND_NOT_FOUND",
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
        friend_result::UNKNOWN => "ERR_FRIEND_ERROR",
        // An unknown code shows nothing — GlobalStrings data-suppression, the same face an
        // absent key wears everywhere else in this client.
        _ => return None,
    })
}

/// How many times the subject's name is pushed with a result's message — **the arity the key's own
/// suffix names**, which is the reference's convention rather than ours: a bare key takes none,
/// `_S` one, `_SS` two.
///
/// `ERR_FRIEND_ONLINE_SS`'s two are the `|Hplayer:%s|h[%s]|h` link's, and GlobalStrings' own
/// comment beside it warns the link is not to be localized. Reading the arity off the key rather
/// than counting `%s` in the resolved text is what keeps a line that has not resolved yet — an
/// install without the string, a name still in flight — from being mistaken for one that takes no
/// name at all.
fn name_pushes(key: &str) -> usize {
    if key.ends_with("_SS") {
        2
    } else if key.ends_with("_S") {
        1
    } else {
        0
    }
}

/// The `CHAT_FLAG_AFK`/`CHAT_FLAG_DND` key a friend row's away tag resolves through — the same
/// pair the chat frame prefixes a speaker's name with. `None` = present and not flagged, which
/// shows nothing.
fn status_flag_key(status: u8) -> Option<&'static str> {
    Some(match status {
        friend_status::AFK => "CHAT_FLAG_AFK",
        friend_status::DND => "CHAT_FLAG_DND",
        _ => return None,
    })
}

/// The social family's packet handlers (in the net handler table since 2312),
/// beside the state they drive ([`crate::ui_duel::net`]'s shape): the friend/ignore lists, the
/// `/who` answer, and the result codes that print their own chat lines. The lines and the Era
/// events fire off the mirror in [`feed_social`] — every one of them needs a NAME, which the
/// feed resolves.
pub(crate) mod net {
    use super::*;
    use benilla_protocol::{SessionEvent, SessionEventKind};

    use crate::net::NetHandlerApp;

    /// Register the family's handlers — called from [`UiSocialPlugin`]. One per kind, plus the
    /// session-end listener.
    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::FriendList, on_friend_list)
            .net_handler(K::IgnoreList, on_ignore_list)
            .net_handler(K::FriendStatus, on_friend_status)
            .net_handler(K::WhoResults, on_who)
            .net_handler(K::Disconnected, on_session_end);
    }

    fn on_friend_list(In(ev): In<SessionEvent>, mut social: ResMut<SocialState>) {
        if let SessionEvent::FriendList { friends } = ev {
            friend_list(&mut social, friends);
        }
    }

    fn on_ignore_list(In(ev): In<SessionEvent>, mut social: ResMut<SocialState>) {
        if let SessionEvent::IgnoreList { guids } = ev {
            ignore_list(&mut social, guids);
        }
    }

    fn on_friend_status(In(ev): In<SessionEvent>, mut social: ResMut<SocialState>) {
        if let SessionEvent::FriendStatus(update) = ev {
            friend_status(&mut social, update);
        }
    }

    fn on_who(In(ev): In<SessionEvent>, mut social: ResMut<SocialState>) {
        if let SessionEvent::WhoResults(results) = ev {
            who(&mut social, results);
        }
    }

    /// The friend/ignore lists and the last `/who` are session state: the
    /// server re-pushes both lists at the next login, and a stale ignore list would silence the
    /// wrong guids after a reconnect renumbers nothing but re-streams everything. The `/who`
    /// sort chain is the one thing that survives — it is per-PROCESS in the reference, not
    /// per-login, which is why this is a `clear_session` and not a `default()`.
    /// A listener on the session end (a second handler on the kind, after the bridge's own teardown).
    fn on_session_end(In(_): In<SessionEvent>, mut social: ResMut<SocialState>) {
        social.clear_session();
    }

    /// `SMSG_FRIEND_LIST`.
    pub(crate) fn friend_list(social: &mut SocialState, friends: Vec<FriendEntry>) {
        social.apply_friend_list(friends);
    }

    /// `SMSG_IGNORE_LIST`.
    pub(crate) fn ignore_list(social: &mut SocialState, guids: Vec<u64>) {
        social.apply_ignore_list(guids);
    }

    /// `SMSG_FRIEND_STATUS`.
    pub(crate) fn friend_status(social: &mut SocialState, update: FriendStatusUpdate) {
        social.apply_friend_status(update);
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
        app.init_resource::<SocialState>().add_systems(
            Update,
            (
                feed::feed_social.in_set(UiFeed),
                feed::drain_social.after(UiInput),
            ),
        );
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

    /// The server sends no fresh list after an add or a remove, so the result codes have to keep
    /// the local list honest — otherwise a friend added stays invisible until the next login.
    #[test]
    fn add_and_remove_results_maintain_the_list() {
        let mut social = SocialState::default();
        social.apply_friend_status(FriendStatusUpdate {
            result: friend_result::ADDED_ONLINE,
            guid: 7,
            online: Some(FriendOnline {
                status: friend_status::ONLINE,
                area: 12,
                level: 60,
                class: 4,
            }),
        });
        assert_eq!(social.friends.len(), 1);
        assert_eq!(social.friends[0].level, 60);

        // The same add arriving twice must not duplicate the row.
        social.apply_friend_status(status(friend_result::ADDED_OFFLINE, 7));
        assert_eq!(social.friends.len(), 1);

        social.apply_friend_status(status(friend_result::REMOVED, 7));
        assert!(social.friends.is_empty());
    }

    /// A presence broadcast patches the row in place — and going offline clears the level/zone,
    /// because the wire stops sending them (a stale "Level 60 Rogue" under an Offline row would
    /// be the client inventing data).
    #[test]
    fn presence_broadcasts_patch_the_row() {
        let mut social = SocialState::default();
        social.apply_friend_list(vec![FriendEntry {
            guid: 7,
            ..Default::default()
        }]);

        social.apply_friend_status(FriendStatusUpdate {
            result: friend_result::ONLINE,
            guid: 7,
            online: Some(FriendOnline {
                status: friend_status::AFK,
                area: 1519,
                level: 42,
                class: 8,
            }),
        });
        assert_eq!(social.friends[0].status, friend_status::AFK);
        assert_eq!(social.friends[0].area, 1519);

        social.apply_friend_status(status(friend_result::OFFLINE, 7));
        assert!(!social.friends[0].is_online());
        assert_eq!(
            (social.friends[0].area, social.friends[0].level),
            (0, 0),
            "offline carries no zone or level"
        );
    }

    /// Removing the selected friend clears the selection rather than leaving it pointing at a
    /// player who is no longer listed.
    #[test]
    fn removing_the_selected_friend_clears_the_selection() {
        let mut social = SocialState::default();
        social.apply_friend_list(vec![FriendEntry {
            guid: 7,
            ..Default::default()
        }]);
        social.selected_friend = 7;
        social.apply_friend_status(status(friend_result::REMOVED, 7));
        assert_eq!(social.selected_friend, 0);
    }

    /// The ignore results maintain the ignore list, and [`SocialState::is_ignored`] is what the
    /// chat/duel gates read.
    #[test]
    fn ignore_results_maintain_the_ignore_list() {
        let mut social = SocialState::default();
        assert!(!social.is_ignored(9));
        social.apply_friend_status(status(friend_result::IGNORE_ADDED, 9));
        assert!(social.is_ignored(9));
        assert!(!social.is_ignored(0), "guid 0 is never ignored");
        social.apply_friend_status(status(friend_result::IGNORE_REMOVED, 9));
        assert!(!social.is_ignored(9));
    }

    /// A refusal changes nothing — but still owes a line.
    #[test]
    fn a_refusal_changes_no_state_but_prints() {
        let mut social = SocialState::default();
        social.apply_friend_status(status(friend_result::ALREADY, 7));
        assert!(social.friends.is_empty());
        assert_eq!(social.pending_lines.len(), 1);
    }

    /// Every result vmangos can send has a line, and the two ADDED codes share one — which is
    /// exactly why 18 results fit the 17 catalog ids `0x104..0x114`.
    #[test]
    fn every_result_code_maps_to_a_line() {
        for result in 0x00..=0x11u8 {
            assert!(
                result_key(result).is_some(),
                "result {result:#04x} has no line"
            );
        }
        assert_eq!(
            result_key(friend_result::ADDED_ONLINE),
            result_key(friend_result::ADDED_OFFLINE),
        );
        assert_eq!(result_key(friend_result::UNKNOWN), Some("ERR_FRIEND_ERROR"));
        assert_eq!(result_key(0x77), None, "an unknown code shows nothing");
        // **The two "Player not found." rows are different keys.** They read identically in enUS
        // and differently in other locales, so this pair is the reason the table names keys at
        // all — no assertion on displayed text could tell them apart.
        assert_eq!(
            result_key(friend_result::NOT_FOUND),
            Some("ERR_FRIEND_NOT_FOUND")
        );
        assert_eq!(
            result_key(friend_result::IGNORE_NOT_FOUND),
            Some("ERR_IGNORE_NOT_FOUND")
        );
    }

    /// A logout drops the lists and the last `/who`, and keeps the **sort chain** — which is
    /// per-process in the reference, not per-login, so a player who left the who list sorted by
    /// level descending finds it that way after a relog.
    #[test]
    fn a_logout_keeps_the_sort_chain_and_drops_everything_else() {
        let mut social = SocialState::default();
        social.apply_friend_list(vec![FriendEntry {
            guid: 7,
            ..Default::default()
        }]);
        social.apply_ignore_list(vec![9]);
        social.who_to_ui = true;
        social.who_sort.promote("level");
        social.who_sort.promote("level"); // descending
        let chain = social.who_sort.clone();

        social.clear_session();
        assert!(social.friends.is_empty());
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

    /// **The arity comes off the key's suffix**, which is the reference's own marker: the online
    /// line's `_SS` takes the name twice (the `|Hplayer:%s|h[%s]|h` link), an `_S` once, and a
    /// bare key none. Read this way rather than by counting `%s` in the resolved sentence, so a
    /// line whose string has not resolved is never mistaken for one that needs no name.
    #[test]
    fn the_key_suffix_is_the_name_arity() {
        assert_eq!(name_pushes("ERR_FRIEND_ONLINE_SS"), 2);
        assert_eq!(name_pushes("ERR_FRIEND_ADDED_S"), 1);
        assert_eq!(name_pushes("ERR_FRIEND_SELF"), 0, "not an _S despite the S");
    }

    /// **Every key this table names resolves in the shipped file, is a catalog row, and its own
    /// `%s` count matches the arity its suffix promises.** The three checks that replace comparing
    /// our copy of each sentence to the file's — and the third is the one the copies never made:
    /// nothing used to join `ERR_FRIEND_ONLINE_SS`'s two holes to the two pushes it gets.
    #[test]
    fn every_key_resolves_and_its_arity_matches_the_string() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let s = benilla_ui::script::UiScript::new().expect("VM");
        s.run(&String::from_utf8_lossy(&src)).expect("runs clean");

        let mut keys: Vec<&str> = (0x00..=0x11u8).filter_map(result_key).collect();
        keys.extend(
            [
                status_flag_key(friend_status::AFK),
                status_flag_key(friend_status::DND),
            ]
            .map(Option::unwrap),
        );
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), 19, "17 result rows plus the two away tags");

        for key in keys {
            let text: String = s.lua().globals().get(key).expect(key);
            assert!(!text.is_empty(), "{key} resolves empty");
            // The away tags are `CHAT_FLAG_*`, not `DisplayError` messages: no catalog row, and
            // none owed — they are a field on a friends-list row, not a line on a surface.
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
