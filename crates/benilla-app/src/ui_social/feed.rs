//! The social VM feed/drain — the systems half of [`super`]: resolve the wire's guids and ids
//! into the display-ready snapshot the FriendsFrame reads, fire the list events on their edges,
//! print the result lines, and turn the Lua-side [`SocialRequest`] intents into their sends.
//!
//! Everything resolved here is resolved *engine-side in the reference too* (`FriendList`'s
//! formatter `0x5ae160` reads the name cache and the race/class/area GameTables before Lua sees a
//! row) — see [`super`]'s module doc.

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

/// `WHO_LIST_FORMAT` / `WHO_LIST_GUILD_FORMAT` / `WHO_NUM_RESULTS(_P1)` — the chat-routed `/who`
/// output. These four keys appear **nowhere** in the reference's FrameXML (exhaustively grepped),
/// which is what identifies them as engine-composed: when `SetWhoToUI` is off, the engine prints
/// the results as chat lines itself. So do we — and, since decision 2045, out of the player's own
/// `GlobalStrings.lua` rather than out of a copy typed here.
///
/// **None of the four is a message-catalog row**, so these lines cannot go out through
/// `Shown::keyed`: an unknown key takes that path's fallback and turns the line red. They are
/// `Shown::unkeyed(MsgKind::Chat, …)`, the same route `/ginfo`'s two templates take (2054).
const WHO_KEYS: [&str; 4] = [
    "WHO_LIST_FORMAT",
    "WHO_LIST_GUILD_FORMAT",
    "WHO_NUM_RESULTS",
    "WHO_NUM_RESULTS_P1",
];

/// The chat-frame threshold on an answer the Who frame did not claim. `SMSG_WHO`'s parser
/// (`0x5adf60`) computes one **print flag** before its record loop (`0x5adf9c`–`0x5adfd5`) and
/// that flag gates both halves: with `SetWhoToUI` set the event always fires; with it clear,
/// `0x5adfca cmp eax,ecx` / `0x5adfcc jl` — signed, `eax = 3` — sends **more than three** rows to
/// `WHO_LIST_UPDATE` (the frame holds them for whenever it opens, printing nothing) and three or
/// fewer to the chat frame, with no event. Four hits go to the frame; three go to chat. The
/// threshold is the literal `3`: the pointer that could override it (`[[0xc2a128]+0x28]`) has one
/// reference image-wide and no writer.
///
/// **`ecx` is the RAW wire display count** (`[ebp-0x14]`), not the 50-capped global the cap at
/// `0x5adf92` writes — a re-implementation must not test its own clamped count and call it the
/// same rule. Ours is [`SocialState::who`]'s length, which is the wire's count uncapped (vmangos
/// sends at most 49), so the two agree. Decision 2030.
const WHO_CHAT_MAX: usize = 3;

/// What the feed last announced, so the Era events fire on edges rather than every frame.
#[derive(Default)]
pub(super) struct FedSocial {
    /// Has the VM been given a snapshot at all yet? The first push always fires the update
    /// events, so a frame loaded after the list arrived still populates.
    seeded: bool,
}

/// Build the display snapshot, push it to the VM, fire the list events, and drain the owed
/// result lines.
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
        // Each owed line carries a catalog key, so the surface AND the sound come from its row —
        // `ERR_FRIEND_ONLINE_SS`'s is `FRIENDJOINGAME`, which a straight chat push could not play.
        let owed: Vec<Shown> = drain_result_lines(&mut social, &names, &commands)
            .iter()
            .filter_map(|e| {
                crate::ui_action::ui_error_text(e, &get).map(|text| Shown::keyed(e.key, text))
            })
            .collect();
        // The row's away tag, through `CHAT_FLAG_AFK`/`_DND` — the chat frame's own pair.
        let away = |status: u8| status_flag_key(status).and_then(&get).unwrap_or_default();
        let (friends, display_order) = friend_rows(&social, &names, &commands, areas, &away);
        (owed, friends, display_order)
    };

    // The owed lines first: one about a friend who just went offline should land before the list
    // update that removes their zone.
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
        // The chain rides along because `SortWho` promotes and re-sorts inside the binding — see
        // its comment in `benilla_ui::script::social`.
        who_sort: social.who_sort.clone(),
    });

    // The three list events (`FriendsFrame_OnEvent`'s own arms). FRIENDLIST_SHOW is the answer to
    // an explicit `ShowFriends()`; a list that simply changed fires FRIENDLIST_UPDATE.
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
    // `SMSG_WHO`'s two exits ([`WHO_CHAT_MAX`]). Note the event is the answer's *only* announcement
    // — `SortWho` fires its own, synchronously, from inside the binding — so a sort no longer
    // re-announces the list a tick later (decision 2030).
    if social.who_dirty {
        social.who_dirty = false;
        if answer_goes_to_the_frame(social.who_to_ui, social.who.len()) {
            script.fire_event("WHO_LIST_UPDATE", Vec::new());
        } else {
            // Nobody is holding the list — the engine prints the results itself, **in wire
            // order**: the per-record line is composed inside the parse loop (`0x5ae0a1`, one
            // call per record) and the `qsort` at `0x5ae0e2` sits past the loop's back-edge, so
            // it reorders only the array `GetWhoInfo` reads, never these lines.
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

/// Which exit an `SMSG_WHO` answer takes: `true` fires `WHO_LIST_UPDATE` and prints nothing,
/// `false` prints the chat lines and fires nothing ([`WHO_CHAT_MAX`]).
fn answer_goes_to_the_frame(to_ui: bool, shown: usize) -> bool {
    to_ui || shown > WHO_CHAT_MAX
}

/// Name every result line whose subject has resolved. A line that needs a name waits for the
/// query (the reference's resolve-then-compose order); one that doesn't is ready at once.
///
/// Whether a name is needed comes off the KEY's `_S`/`_SS` suffix ([`name_pushes`]) rather than
/// off the resolved text: the text may not be there yet, and "no `%s` in it" and "no string for
/// it" must not look the same.
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
        // A named line with no subject (the server answers a failed lookup with guid 0) can never
        // resolve — print nothing rather than a starved template, or hold it forever.
        if update.guid == 0 {
            continue;
        }
        match names.resolve(update.guid, commands).map(str::to_string) {
            // The same name, pushed as many times as the key's arity says — which is what the
            // reference does for the `|Hplayer:%s|h[%s]|h` link.
            Some(name) => ready.push(UiError::strings(key, &vec![name.as_str(); pushes])),
            None => still_pending.push(update),
        }
    }
    social.pending_lines = still_pending;
    ready
}

/// The chat-routed `/who` output (module doc's four engine-only templates): one line per row **in
/// the order given**, and the `WHO_NUM_RESULTS` total **last**.
///
/// The order is the parser's, not a presentation choice: `0x5ae0a1` composes a record's line
/// inside the loop that reads it, and the summary block `0x5ae0f1`–`0x5ae12a` sits after the
/// loop's back-edge.
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
        // The templates interleave `%s` and `%d` and are positional-by-order, not indexed: name,
        // name, level, race, class, [guild,] zone. The shared filler walks both specifiers in one
        // pass, which is what retired this file's own replace-first token walk (2045).
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
    // The `_P1` plural twin for anything but exactly one — `GetText`'s own rule, so a zero total
    // reads "0 players total" (see `benilla_ui::script::tooltip::plural_template`).
    let total_key = if total == 1 { WHO_KEYS[2] } else { WHO_KEYS[3] };
    if let Some(template) = get(total_key) {
        lines.extend(chat(fill(&template, &[Arg::D(i64::from(total))])));
    }
    lines
}

/// The friend rows in display order (name-sorted), plus the guid order that produced them so the
/// drain can map a row index back to a player.
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
                    // An offline friend has no class/zone on the wire; leaving them empty is what
                    // makes the frame print its "Offline" template instead of inventing values.
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

    // Name order, with the not-yet-resolved rows last so an in-flight name query doesn't park an
    // empty row at the top of the list.
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

/// The `/who` rows, resolved and then ordered by the sort chain — the reference's own
/// `qsort 0x5ae0e2` on every fresh answer, plus whatever a header click promoted since.
///
/// Recomputed from the wire mirror rather than kept sorted in place, because the comparator's
/// three DBC arms compare **resolved names** (`ChrClasses`/`ChrRaces`/`AreaTable`), which only
/// exist on this side of [`who_row`]. The result is the same either way: the chain always ends
/// with the name key somewhere in it, and no two rows of one answer share a name, so the order is
/// total — re-deriving it per frame lands on the identical list.
fn who_rows(social: &SocialState, areas: Option<&AreaTableCatalog>) -> Vec<WhoInfo> {
    let mut rows: Vec<WhoInfo> = social.who.iter().map(|e| who_row(e, areas)).collect();
    social.who_sort.sort(&mut rows);
    rows
}

/// One `/who` row, ids resolved. Note the Lua API returns race *before* class while the wire
/// carries class first — the swap happens here, once.
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

/// The 1-based row a guid occupies in the shown order, `0` when it isn't shown — the reference's
/// guid→index conversion (`GetSelectedFriend` `0x5ae510`).
fn index_of(order: &[u64], guid: u64) -> u32 {
    if guid == 0 {
        return 0;
    }
    order
        .iter()
        .position(|g| *g == guid)
        .map_or(0, |i| i as u32 + 1)
}

/// Turn the Era API's social intents into their sends. Every "by index" intent resolves through
/// the display order the feed just published, and every "by name" one through the list's own
/// resolved names — because the wire removes by **guid** (see [`super`]'s module doc).
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
                // `0x5ae67c`: Friends acknowledged right before the send (1976).
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
                // `/ignore <name>`: un-ignore if they're already on the list, else ignore them.
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
            // The click the binding already applied to the VM's copy of the chain, applied to
            // ours — so the next push agrees. No `who_dirty`: `SortWho` fired `WHO_LIST_UPDATE`
            // synchronously, inside the binding, and the reference fires it exactly once per
            // click (decision 2030).
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

/// The guid of the listed player called `name`, case-insensitively — the name→guid direction the
/// `/removefriend` and `/unignore` verbs need before they can send anything.
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

    /// **B365, app side.** A fresh `SMSG_WHO` answer is presented in the sort chain's order, not
    /// the server's — the reference qsorts inside the parse itself (`0x5ae0e2`), so a list sorted
    /// by level stays sorted by level when the next `/who` lands. And a header click's intent
    /// re-orders the *next* push without needing a fresh answer.
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

        // A Name click: ascending, and case-folded — `erdrin` sorts among the capitals, not after
        // them as a byte compare would put it.
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

    /// The two exits of the `SMSG_WHO` parse. The threshold is what makes a broad `/who` typed
    /// with the window shut go quiet instead of dumping fifty lines into the chat frame.
    #[test]
    fn a_small_answer_goes_to_chat_and_a_large_one_to_the_frame() {
        assert!(!answer_goes_to_the_frame(false, 0));
        assert!(!answer_goes_to_the_frame(false, 3), "three still print");
        assert!(answer_goes_to_the_frame(false, 4), "four go to the frame");
        // With the frame open it is always the frame's, however few hits there are.
        for shown in 0..=4 {
            assert!(answer_goes_to_the_frame(true, shown));
        }
    }

    /// The chat lines come out in **wire** order with the total **last** — and they are the one
    /// half of the who list the sort chain does not touch, however hard that reads as an
    /// inconsistency.
    ///
    /// The reference composes a record's line inside the loop that parses it (`0x5ae0a1`), and
    /// the `qsort` that orders the array sits past that loop's back-edge (`0x5ae0e2`) — so the
    /// lines are already gone by the time anything is sorted, and only the array `GetWhoInfo`
    /// reads is ordered. Sorting them "for consistency" is the plausible wrong answer, and it was
    /// this client's until the call site was read.
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
        // Against the real shipped templates: these four keys are engine-composed, so there is no
        // FrameXML call site to read them off and a stub would only assert our own guess back.
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

    /// The comparator's "missing DBC row" marker is the **empty** name, and it must stay that
    /// way: `who_row` leaves an unresolvable class/race/zone empty, and the chain ties on it
    /// rather than ordering it (the arm jumps to the loop's `inc esi` at `0x5adbb2`).
    ///
    /// **This test is a tripwire.** The reference's `GetWhoInfo 0x5ad6e0` substitutes the
    /// localized `"UNKNOWN"` on those same three legs, so the two sides deliberately
    /// disagree: the cell reads UNKNOWN and the row sorts as if the column were not there. If
    /// this client ever adopts that substitution — it should; ours shows an empty cell today —
    /// the miss has to travel to the comparator by some other route than the string, or the tie
    /// silently becomes an alphabetical sort on the word "Unknown".
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

    /// A row index maps back through the *shown* order, and 0/past-the-end map to nothing — the
    /// guard that keeps a stale click from removing a bystander.
    #[test]
    fn row_indices_map_through_the_shown_order() {
        let order = [11u64, 22, 33];
        assert_eq!(row_guid(&order, 1), Some(11));
        assert_eq!(row_guid(&order, 3), Some(33));
        assert_eq!(row_guid(&order, 0), None);
        assert_eq!(row_guid(&order, 4), None);
    }

    /// Selection survives a re-order because it is stored as a guid: the same player keeps the
    /// highlight even when the row under them moves.
    #[test]
    fn selection_follows_the_player_not_the_row() {
        assert_eq!(index_of(&[11, 22, 33], 22), 2);
        assert_eq!(index_of(&[22, 11, 33], 22), 1, "same player, new row");
        assert_eq!(index_of(&[11, 33], 22), 0, "no longer listed");
        assert_eq!(index_of(&[11, 22], 0), 0, "nothing selected");
    }

    /// **The who templates interleave `%s` and `%d`**, and the guilded one takes a seventh fill
    /// the other does not — so the row's fields have to reach them in wire order, through one
    /// walk that speaks both specifiers. That walk is `benilla_ui::strings::fill` now; this file
    /// used to carry its own `replace_first_token`, one of the eight copies decision 2045 retired.
    ///
    /// Against the shipped templates, not a stub: these four keys are engine-composed, so there
    /// is no FrameXML call site to read them off and a stub would assert our own guess back.
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

    /// An offline friend shows no class or zone — the wire sends neither, so the row must not
    /// carry the values it had while online.
    #[test]
    fn an_offline_row_carries_no_class_or_zone() {
        let entry = benilla_protocol::messages::FriendEntry {
            guid: 7,
            status: friend_status::OFFLINE,
            area: 12,
            level: 60,
            class: 4,
        };
        // `who_row`'s sibling path, exercised through the same resolution rules the feed uses.
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
