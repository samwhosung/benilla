//! The social feed and drain: the snapshot the FriendsFrame reads, the list events, the result
//! lines, and the sends for the Lua-side [`SocialRequest`] intents. The reference resolves names,
//! races, classes and zones engine-side too, before Lua sees a row (`0x5ae160`).

use benilla_formats::AreaTableCatalog;
use benilla_protocol::messages::WhoEntry;
use benilla_ui::script::{FriendInfo, SocialRequest, SocialState as VmSocial, UiScript, WhoInfo};
use bevy::prelude::*;

use crate::area::AreaTableRes;
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands};
use crate::ui_action::{MessageSink, Shown, UiError};
use crate::ui_unit::{class_names, race_names};

use super::{name_pushes, result_key, status_flag_key, SocialState};

/// The chat-routed `/who` templates. FrameXML never names them: the engine prints these lines
/// itself. None is a message-catalog row, so they go out through `Shown::unkeyed`, since
/// `Shown::keyed` turns an unknown key's line red.
const WHO_KEYS: [&str; 4] = [
    "WHO_LIST_FORMAT",
    "WHO_LIST_GUILD_FORMAT",
    "WHO_NUM_RESULTS",
    "WHO_NUM_RESULTS_P1",
];

/// The most rows that print to chat when the Who frame does not claim an answer. `SMSG_WHO`'s
/// parser (`0x5adf60`) sends more than three rows to `WHO_LIST_UPDATE` and three or fewer to chat
/// lines, with no event (`0x5adfca`); with `SetWhoToUI` set the event always fires. It compares the
/// raw wire count, not the 50-capped copy (`0x5adf92`).
const WHO_CHAT_MAX: usize = 3;

/// What the feed last announced, so the list events fire on edges.
#[derive(Default)]
pub(super) struct FedSocial {
    /// Whether the VM has had a snapshot; the first push fires the list events regardless.
    seeded: bool,
}

/// Push the display snapshot to the VM, fire the list events, and print the owed result lines.
pub(super) fn feed_social(
    script: Option<NonSendMut<UiScript>>,
    mut social: ResMut<SocialState>,
    names: Res<NameCache>,
    areas: Option<Res<AreaTableRes>>,
    commands: Res<NetCommands>,
    mut sink: MessageSink,
    mut fed: Local<crate::ui_script::VmMemo<FedSocial>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let fed = fed.get(&script);
    let areas = areas.as_deref().map(|a| &a.0);

    // Everything that reads the VM's own string table, resolved under one borrow of it.
    let (owed, friends, display_order) = {
        let get = |key: &str| script.lua().globals().get::<String>(key).ok();
        // Keyed, so the surface and the sound come from the catalog row (`ERR_FRIEND_ONLINE_SS`
        // plays `FRIENDJOINGAME`).
        let owed: Vec<Shown> = drain_result_lines(&mut social, &names, &commands)
            .iter()
            .filter_map(|e| {
                crate::ui_action::ui_error_text(e, &get).map(|text| Shown::keyed(e.key, text))
            })
            .collect();
        let away = |status: u8| status_flag_key(status).and_then(&get).unwrap_or_default();
        let (friends, display_order) = friend_rows(&social, &names, &commands, areas, &away);
        (owed, friends, display_order)
    };

    // The owed lines first, so a friend's offline line lands before the update that clears their
    // zone.
    crate::ui_action::show_messages(&mut script, &mut sink, "ui_social", owed);

    let (ignores, ignore_order) = ignore_rows(&social, &names, &commands);
    let who = who_rows(&social, areas);

    let selected_friend = index_of(&display_order, social.selected_friend);
    let selected_ignore = index_of(&ignore_order, social.selected_ignore);
    social.display_order = display_order;
    social.ignore_display_order = ignore_order;

    script.set_social(VmSocial {
        friends,
        selected_friend,
        ignores,
        selected_ignore,
        who,
        who_total: social.who_total,
        // The chain rides along: `SortWho` re-sorts inside the binding.
        who_sort: social.who_sort.clone(),
    });

    let first = !fed.seeded;
    fed.seeded = true;
    if social.friends_dirty || first {
        social.friends_dirty = false;
        let event = if std::mem::take(&mut social.friends_show_pending) {
            "FRIENDLIST_SHOW"
        } else {
            "FRIENDLIST_UPDATE"
        };
        script.fire_event(event, Vec::new());
    }
    if social.ignores_dirty || first {
        social.ignores_dirty = false;
        script.fire_event("IGNORELIST_UPDATE", Vec::new());
    }
    // `SMSG_WHO`'s two exits ([`WHO_CHAT_MAX`]); a sort fires its own event inside `SortWho`.
    if social.who_dirty {
        social.who_dirty = false;
        if answer_goes_to_the_frame(social.who_to_ui, social.who.len()) {
            script.fire_event("WHO_LIST_UPDATE", Vec::new());
        } else {
            // In wire order: each line is composed inside the parse loop (`0x5ae0a1`), before the
            // `qsort` at `0x5ae0e2` orders the array `GetWhoInfo` reads.
            let printed = {
                let get = |key: &str| script.lua().globals().get::<String>(key).ok();
                let wire_order: Vec<WhoInfo> =
                    social.who.iter().map(|e| who_row(e, areas)).collect();
                who_lines(&wire_order, social.who_total, &get)
            };
            crate::ui_action::show_messages(&mut script, &mut sink, "ui_social", printed);
        }
    }
}

/// Whether an `SMSG_WHO` answer fires `WHO_LIST_UPDATE` (true) or prints chat lines (false).
fn answer_goes_to_the_frame(to_ui: bool, shown: usize) -> bool {
    to_ui || shown > WHO_CHAT_MAX
}

/// Name every result line whose subject has resolved: a line that needs a name waits for the name
/// query, as the reference resolves before it composes.
fn drain_result_lines(
    social: &mut SocialState,
    names: &NameCache,
    commands: &NetCommands,
) -> Vec<UiError> {
    let mut still_pending = Vec::new();
    let mut ready = Vec::new();
    for update in std::mem::take(&mut social.pending_lines) {
        let Some(key) = result_key(update.result) else {
            continue; // an unknown code shows nothing
        };
        let pushes = name_pushes(key);
        if pushes == 0 {
            ready.push(UiError::key(key));
            continue;
        }
        // A named line with guid 0 (the server's answer to a failed lookup) can never resolve.
        if update.guid == 0 {
            continue;
        }
        match names.resolve(update.guid, commands).map(str::to_string) {
            // The name, once per `%s` the key's arity names (twice for the player link).
            Some(name) => ready.push(UiError::strings(key, &vec![name.as_str(); pushes])),
            None => still_pending.push(update),
        }
    }
    social.pending_lines = still_pending;
    ready
}

/// The chat-routed `/who` output: one line per row in the order given, then the total (`0x5ae0f1`).
fn who_lines(rows: &[WhoInfo], total: u32, get: &dyn Fn(&str) -> Option<String>) -> Vec<Shown> {
    use benilla_ui::strings::{fill, Arg};

    let chat = |text: String| {
        (!text.is_empty()).then(|| Shown::unkeyed(benilla_ui::messages::MsgKind::Chat, text))
    };
    let mut lines = Vec::with_capacity(rows.len() + 1);
    for row in rows {
        let key = if row.guild.is_empty() {
            WHO_KEYS[0]
        } else {
            WHO_KEYS[1]
        };
        let Some(template) = get(key) else {
            continue; // no string, no line
        };
        // The templates fill in order, `%s` and `%d` mixed: name, name, level, race, class,
        // [guild,] zone.
        let mut args = vec![
            Arg::S(&row.name),
            Arg::S(&row.name),
            Arg::D(i64::from(row.level)),
            Arg::S(&row.race),
            Arg::S(&row.class),
        ];
        if !row.guild.is_empty() {
            args.push(Arg::S(&row.guild));
        }
        args.push(Arg::S(&row.zone));
        lines.extend(chat(fill(&template, &args)));
    }
    // `_P1` for any total but one, `GetText`'s plural rule, so zero reads "0 players total".
    let total_key = if total == 1 { WHO_KEYS[2] } else { WHO_KEYS[3] };
    if let Some(template) = get(total_key) {
        lines.extend(chat(fill(&template, &[Arg::D(i64::from(total))])));
    }
    lines
}

/// The friend rows sorted by name, and their guids in the same order for the drain.
fn friend_rows(
    social: &SocialState,
    names: &NameCache,
    commands: &NetCommands,
    areas: Option<&AreaTableCatalog>,
    away: &dyn Fn(u8) -> String,
) -> (Vec<FriendInfo>, Vec<u64>) {
    let mut rows: Vec<(u64, FriendInfo)> = social
        .friends
        .iter()
        .map(|entry| {
            let name = names
                .resolve(entry.guid, commands)
                .map(str::to_string)
                .unwrap_or_default();
            let online = entry.is_online();
            (
                entry.guid,
                FriendInfo {
                    name,
                    level: entry.level,
                    // An offline friend has no class or zone on the wire; empty, the frame prints
                    // its Offline template.
                    class: online
                        .then(|| class_names(entry.class as u8))
                        .flatten()
                        .map(|(display, _)| display.to_string())
                        .unwrap_or_default(),
                    area: online
                        .then(|| areas.and_then(|a| a.name(entry.area)))
                        .flatten()
                        .unwrap_or_default()
                        .to_string(),
                    connected: online,
                    status: away(entry.status),
                },
            )
        })
        .collect();

    // By name, unresolved rows last, so a pending name query parks no empty row on top.
    rows.sort_by(|(_, a), (_, b)| {
        a.name
            .is_empty()
            .cmp(&b.name.is_empty())
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    rows.into_iter().map(|(guid, row)| (row, guid)).unzip()
}

/// The ignore rows: names only, same ordering rule.
fn ignore_rows(
    social: &SocialState,
    names: &NameCache,
    commands: &NetCommands,
) -> (Vec<String>, Vec<u64>) {
    let mut rows: Vec<(u64, String)> = social
        .ignores
        .iter()
        .map(|guid| {
            (
                *guid,
                names
                    .resolve(*guid, commands)
                    .map(str::to_string)
                    .unwrap_or_default(),
            )
        })
        .collect();
    rows.sort_by(|(_, a), (_, b)| {
        a.is_empty()
            .cmp(&b.is_empty())
            .then_with(|| a.to_lowercase().cmp(&b.to_lowercase()))
    });
    rows.into_iter().map(|(guid, name)| (name, guid)).unzip()
}

/// The `/who` rows, resolved, then sorted by the chain as the reference sorts every answer
/// (`0x5ae0e2`); the comparator compares resolved names, so the sort runs after [`who_row`].
fn who_rows(social: &SocialState, areas: Option<&AreaTableCatalog>) -> Vec<WhoInfo> {
    let mut rows: Vec<WhoInfo> = social.who.iter().map(|e| who_row(e, areas)).collect();
    social.who_sort.sort(&mut rows);
    rows
}

/// One `/who` row, ids resolved; the wire carries class before race, the Lua API race first.
fn who_row(entry: &WhoEntry, areas: Option<&AreaTableCatalog>) -> WhoInfo {
    WhoInfo {
        name: entry.name.clone(),
        guild: entry.guild.clone(),
        level: entry.level,
        race: race_names(entry.race as u8)
            .map(|(display, _)| display)
            .unwrap_or_default()
            .to_string(),
        class: class_names(entry.class as u8)
            .map(|(display, _)| display)
            .unwrap_or_default()
            .to_string(),
        zone: areas
            .and_then(|a| a.name(entry.zone))
            .unwrap_or_default()
            .to_string(),
    }
}

/// The 1-based row of a guid in the shown order, 0 when absent, as `GetSelectedFriend`
/// (`0x5ae510`) converts it.
fn index_of(order: &[u64], guid: u64) -> u32 {
    if guid == 0 {
        return 0;
    }
    order
        .iter()
        .position(|g| *g == guid)
        .map_or(0, |i| i as u32 + 1)
}

/// Turn the Lua API's social intents into their sends. By-index intents resolve through the order
/// the feed published, by-name ones through the resolved names, since the wire removes by guid.
pub(super) fn drain_social(
    script: Option<NonSendMut<UiScript>>,
    mut social: ResMut<SocialState>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    areas: Option<Res<AreaTableRes>>,
    mut tutorials: Option<MessageWriter<crate::tutorial::TutorialEvent>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let requests = script.take_social_requests();
    if requests.is_empty() {
        return;
    }
    for request in requests {
        match request {
            SocialRequest::RefreshFriends => {
                social.friends_show_pending = true;
                let _ = commands.0.send(ClientCommand::FriendListRequest);
            }
            SocialRequest::AddFriend(name) => {
                // `0x5ae67c`: the Friends tutorial is acknowledged just before the send.
                if let Some(t) = tutorials.as_mut() {
                    t.write(crate::tutorial::TutorialEvent::Acknowledge {
                        id: crate::tutorial::id::FRIENDS,
                    });
                }
                let _ = commands.0.send(ClientCommand::AddFriend { name });
            }
            SocialRequest::SetLookingForGroup { slots, comment } => {
                let _ = commands
                    .0
                    .send(ClientCommand::SetLookingForGroup { slots, comment });
            }
            SocialRequest::RemoveFriendIndex(index) => {
                if let Some(guid) = row_guid(&social.display_order, index) {
                    let _ = commands.0.send(ClientCommand::DelFriend { guid });
                }
            }
            SocialRequest::RemoveFriendName(name) => {
                if let Some(guid) = guid_named(&social.display_order, &name, &names) {
                    let _ = commands.0.send(ClientCommand::DelFriend { guid });
                }
            }
            SocialRequest::AddIgnore(name) => {
                let _ = commands.0.send(ClientCommand::AddIgnore { name });
            }
            SocialRequest::DelIgnore(name) => {
                if let Some(guid) = guid_named(&social.ignore_display_order, &name, &names) {
                    let _ = commands.0.send(ClientCommand::DelIgnore { guid });
                }
            }
            SocialRequest::ToggleIgnore(name) => {
                // `/ignore <name>`: un-ignore someone listed, else ignore them.
                match guid_named(&social.ignore_display_order, &name, &names) {
                    Some(guid) => {
                        let _ = commands.0.send(ClientCommand::DelIgnore { guid });
                    }
                    None => {
                        let _ = commands.0.send(ClientCommand::AddIgnore { name });
                    }
                }
            }
            SocialRequest::SelectFriend(index) => {
                social.selected_friend = row_guid(&social.display_order, index).unwrap_or(0);
            }
            SocialRequest::SelectIgnore(index) => {
                social.selected_ignore = row_guid(&social.ignore_display_order, index).unwrap_or(0);
            }
            SocialRequest::Who(filter) => {
                let request = super::who_query(&filter, areas.as_deref().map(|a| &a.0));
                let _ = commands.0.send(ClientCommand::Who {
                    request: Box::new(request),
                });
            }
            // Mirror the click the binding applied to the VM's chain. No `who_dirty`: `SortWho`
            // already fired `WHO_LIST_UPDATE`, once per click as the reference does.
            SocialRequest::SortWho(sort_type) => social.who_sort.promote(&sort_type),
            SocialRequest::SetWhoToUi(on) => social.who_to_ui = on,
        }
    }
}

/// The guid at a 1-based display row.
fn row_guid(order: &[u64], index: u32) -> Option<u64> {
    usize::try_from(index.checked_sub(1)?)
        .ok()
        .and_then(|i| order.get(i))
        .copied()
}

/// The guid of the listed player called `name`, case-insensitively.
fn guid_named(order: &[u64], name: &str, names: &NameCache) -> Option<u64> {
    order.iter().copied().find(|guid| {
        names
            .peek(*guid)
            .is_some_and(|n| n.eq_ignore_ascii_case(name))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::{friend_status, WhoResults};

    fn entry(name: &str, level: u32, guild: &str) -> WhoEntry {
        WhoEntry {
            name: name.to_string(),
            guild: guild.to_string(),
            level,
            class: 1,
            race: 1,
            zone: 12,
        }
    }

    fn names(rows: &[WhoInfo]) -> Vec<&str> {
        rows.iter().map(|r| r.name.as_str()).collect()
    }

    /// The reference sorts inside the parse (`0x5ae0e2`), so a list sorted by level stays sorted
    /// when the next `/who` lands.
    #[test]
    fn the_answer_is_presented_in_the_sort_chains_order() {
        let mut social = SocialState::default();
        social.apply_who(WhoResults {
            displayed: 3,
            total: 3,
            entries: vec![
                entry("Galas", 60, ""),
                entry("erdrin", 12, ""),
                entry("Bruk", 60, ""),
            ],
        });

        // The seeded chain is zone → level → class → group → name → …: one zone and one class
        // here, so it resolves on level, then name.
        assert_eq!(
            names(&who_rows(&social, None)),
            ["erdrin", "Bruk", "Galas"],
            "wire order was Galas, erdrin, Bruk"
        );

        // A Name click: ascending and case-folded, so `erdrin` sorts among the capitals.
        social.who_sort.promote("name");
        assert_eq!(names(&who_rows(&social, None)), ["Bruk", "erdrin", "Galas"]);

        // The same click again reverses.
        social.who_sort.promote("name");
        assert_eq!(names(&who_rows(&social, None)), ["Galas", "erdrin", "Bruk"]);

        // A fresh answer arrives into that same chain.
        social.apply_who(WhoResults {
            displayed: 2,
            total: 2,
            entries: vec![entry("Aaa", 1, ""), entry("Zzz", 1, "")],
        });
        assert_eq!(names(&who_rows(&social, None)), ["Zzz", "Aaa"]);
    }

    #[test]
    fn a_small_answer_goes_to_chat_and_a_large_one_to_the_frame() {
        assert!(!answer_goes_to_the_frame(false, 0));
        assert!(!answer_goes_to_the_frame(false, 3), "three still print");
        assert!(answer_goes_to_the_frame(false, 4), "four go to the frame");
        // With the frame open, every answer is the frame's.
        for shown in 0..=4 {
            assert!(answer_goes_to_the_frame(true, shown));
        }
    }

    #[test]
    fn the_chat_lines_come_out_in_wire_order_with_the_total_last() {
        let mut social = SocialState::default();
        social.apply_who(WhoResults {
            displayed: 2,
            total: 2,
            entries: vec![entry("Zzz", 60, ""), entry("Aaa", 12, "")],
        });
        // A chain that would order them the other way round, to prove it is not consulted.
        social.who_sort.promote("name");
        assert_eq!(
            names(&who_rows(&social, None)),
            ["Aaa", "Zzz"],
            "the frame's order"
        );

        let wire: Vec<WhoInfo> = social.who.iter().map(|e| who_row(e, None)).collect();
        // The shipped templates: FrameXML never names these keys, so a stub would echo a guess.
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let vm = benilla_ui::script::UiScript::new().expect("VM");
        vm.run(&String::from_utf8_lossy(&src)).expect("runs clean");
        let lines: Vec<String> = who_lines(&wire, social.who_total, &|key| {
            vm.lua().globals().get::<String>(key).ok()
        })
        .iter()
        .map(|s| s.text().to_string())
        .collect();

        assert_eq!(lines.len(), 3, "one per row plus the total: {lines:?}");
        assert!(
            lines[0].contains("Zzz"),
            "wire order, not the chain's: {lines:?}"
        );
        assert!(lines[1].contains("Aaa"), "{lines:?}");
        assert_eq!(lines[2], "2 players total", "the summary comes last");
    }

    /// The comparator ties on an unresolved name (`0x5adbb2`). The reference's `GetWhoInfo`
    /// (`0x5ad6e0`) shows `UNKNOWN` where benilla shows an empty cell; adopting it must carry the
    /// miss to the comparator some other way than the string.
    #[test]
    fn an_unresolvable_id_leaves_the_name_empty_for_the_comparator() {
        let row = who_row(
            &WhoEntry {
                name: "Nobody".into(),
                guild: String::new(),
                level: 1,
                class: 200,
                race: 200,
                zone: 999_999,
            },
            None,
        );
        assert_eq!(
            (row.class.as_str(), row.race.as_str(), row.zone.as_str()),
            ("", "", "")
        );
    }

    #[test]
    fn row_indices_map_through_the_shown_order() {
        let order = [11u64, 22, 33];
        assert_eq!(row_guid(&order, 1), Some(11));
        assert_eq!(row_guid(&order, 3), Some(33));
        assert_eq!(row_guid(&order, 0), None);
        assert_eq!(row_guid(&order, 4), None);
    }

    #[test]
    fn selection_follows_the_player_not_the_row() {
        assert_eq!(index_of(&[11, 22, 33], 22), 2);
        assert_eq!(index_of(&[22, 11, 33], 22), 1, "same player, new row");
        assert_eq!(index_of(&[11, 33], 22), 0, "no longer listed");
        assert_eq!(index_of(&[11, 22], 0), 0, "nothing selected");
    }

    /// Against the shipped strings: the guilded template takes a seventh fill.
    #[test]
    fn who_lines_fill_both_specifiers_in_wire_order() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let vm = benilla_ui::script::UiScript::new().expect("VM");
        vm.run(&String::from_utf8_lossy(&src)).expect("runs clean");
        let get = |key: &str| vm.lua().globals().get::<String>(key).ok();

        let row =
            |name: &str, level: u32, guild: &str, race: &str, class: &str, zone: &str| WhoInfo {
                name: name.into(),
                level,
                guild: guild.into(),
                race: race.into(),
                class: class.into(),
                zone: zone.into(),
            };
        let lines: Vec<String> = who_lines(
            &[
                row("Tigole", 40, "Legacy", "Human", "Rogue", "Westfall"),
                row("Solo", 5, "", "Dwarf", "Priest", "Coldridge Valley"),
            ],
            2,
            &get,
        )
        .iter()
        .map(|s| s.text().to_string())
        .collect();

        assert_eq!(
            lines[0],
            "|Hplayer:Tigole|h[Tigole]|h: Level 40 Human Rogue <Legacy> - Westfall"
        );
        assert_eq!(
            lines[1], "|Hplayer:Solo|h[Solo]|h: Level 5 Dwarf Priest - Coldridge Valley",
            "the unguilded template skips the guild fill"
        );
    }

    #[test]
    fn an_offline_row_carries_no_class_or_zone() {
        let entry = benilla_protocol::messages::FriendEntry {
            guid: 7,
            status: friend_status::OFFLINE,
            area: 12,
            level: 60,
            class: 4,
        };
        // The class resolution `friend_rows` uses.
        let online = entry.is_online();
        assert!(!online);
        let class = online
            .then(|| class_names(entry.class as u8))
            .flatten()
            .map(|(d, _)| d.to_string())
            .unwrap_or_default();
        assert_eq!(class, "");
    }
}
