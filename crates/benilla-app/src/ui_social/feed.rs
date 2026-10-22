//! The social feed: the snapshot the FriendsFrame reads, the list events and the `/who` lines. The
//! reference resolves races, classes and zones engine-side too, before Lua sees a row (`0x5ae160`).

use benilla_formats::AreaTableCatalog;
use benilla_protocol::messages::WhoEntry;
use benilla_ui::script::{FriendInfo, IgnoreInfo, SocialState as VmSocial, WhoInfo};
use bevy::prelude::*;

use crate::area::AreaTableRes;
use crate::names::NameCache;
use crate::net::NetCommands;
use crate::script_calls::VmWith;
use crate::ui_action::{show_now, ui_error_text, MessageSink, Shown};
use crate::ui_unit::{NameTables, UnitNames};

use super::{row_of, FriendSlot, Heard, ListEvent, Lists, SocialState, Step};

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
const WHO_CHAT_MAX: u32 = 3;

/// What the feed last announced, so the list events fire on edges.
#[derive(Default)]
pub(super) struct FedSocial {
    /// Whether the VM has had a snapshot; the first push fires the list events regardless.
    seeded: bool,
    /// Whether the VM has been given the dungeon table.
    lfg_pushed: bool,
}

/// Replay what the handlers did that Lua hears, in their order: each list event and line with the
/// lists as it left them pushed and swapped into the engine, so its listeners read, and their verbs
/// act on, the list as that packet or callback did (`0x5ae867`, `0x5ae92d`, `0x496720`, inside the
/// dispatch); then push the lists as they stand, and the `/who` answer's exit.
#[allow(clippy::too_many_arguments)] // the snapshot's sources, one per table it resolves through
pub(super) fn feed_social(
    mut vm: VmWith<ResMut<'static, SocialState>>,
    names: Res<NameCache>,
    areas: Option<Res<AreaTableRes>>,
    unit_names: NameTables,
    dungeons: Option<Res<super::LfgDungeonsRes>>,
    commands: Res<NetCommands>,
    mut sink: MessageSink,
    mut fed: Local<crate::ui_script::VmMemo<FedSocial>>,
) {
    let areas = areas.as_deref().map(|a| &a.0);
    let steps = std::mem::take(&mut vm.own().steps);
    for Step { mut lists, heard } in steps {
        vm.turn(|script, mut social| {
            if let Some(script) = script {
                script.set_social(view(&lists, &social, areas, unit_names.get()));
            }
            social.swap_lists(&mut lists);
        });
        vm.run(|script| {
            let Some(script) = script else {
                return;
            };
            match heard {
                Heard::Event(ListEvent::Friends) => {
                    script.fire_event("FRIENDLIST_UPDATE", Vec::new());
                }
                Heard::Event(ListEvent::Ignores) => {
                    script.fire_event("IGNORELIST_UPDATE", Vec::new());
                }
                Heard::Line(line) => {
                    let get = |key: &str| script.lua().globals().get::<String>(key).ok();
                    let text = ui_error_text(&line, &get).unwrap_or_default();
                    show_now(script, &mut sink, "ui_social", Shown::keyed(line.key, text));
                }
            }
        });
        vm.own().swap_lists(&mut lists);
    }
    let cache = super::net::CacheNames {
        names: &names,
        net: &commands,
    };
    vm.feed(|script, mut social, fires| {
        let Some(script) = script else {
            return;
        };
        let fed = fed.get(script);
        if !fed.lfg_pushed {
            if let Some(d) = dungeons.as_deref() {
                script.set_lfg_dungeons(d.0.clone());
                fed.lfg_pushed = true;
            }
        }
        script.set_social(view(
            &social.lists(&cache),
            &social,
            areas,
            unit_names.get(),
        ));

        // A new VM's first feed stands for every list's signal.
        if !fed.seeded {
            fed.seeded = true;
            fires.post("FRIENDLIST_UPDATE", Vec::new());
            fires.post("IGNORELIST_UPDATE", Vec::new());
        }
        // `SMSG_WHO`'s two exits ([`WHO_CHAT_MAX`]); a sort fires its own event inside `SortWho`.
        if let Some(raw) = social.who_answer.as_ref().map(|a| a.raw) {
            if answer_goes_to_the_frame(script.who_to_ui(), raw) {
                fires.post("WHO_LIST_UPDATE", Vec::new());
            } else {
                let printed = {
                    let get = |key: &str| script.lua().globals().get::<String>(key).ok();
                    printed_lines(&social, areas, unit_names.get(), &get)
                };
                crate::ui_action::show_messages(script, &mut sink, "ui_social", printed);
            }
            social.who_answer = None;
        }
    })
}

/// The chat lines the last `/who` answer prints when it goes to chat: in wire order, each composed
/// inside the parse loop (`0x5ae0a1`) before the `qsort` at `0x5ae0e2` orders the array
/// `GetWhoInfo` reads, then the count line with the raw count (`0x5ae0f1`).
pub(super) fn printed_lines(
    social: &SocialState,
    areas: Option<&AreaTableCatalog>,
    names: UnitNames<'_>,
    get: &dyn Fn(&str) -> Option<String>,
) -> Vec<Shown> {
    let Some(answer) = &social.who_answer else {
        return Vec::new();
    };
    let wire_order: Vec<WhoInfo> = answer
        .wire
        .iter()
        .map(|e| who_row(e, areas, names))
        .collect();
    who_lines(&wire_order, answer.raw, get)
}

/// The VM's snapshot of `lists`, with the selections as the engine holds them now and the `/who`
/// answer.
fn view(
    lists: &Lists,
    social: &SocialState,
    areas: Option<&AreaTableCatalog>,
    names: UnitNames<'_>,
) -> VmSocial {
    VmSocial {
        friends: friend_rows(lists, areas, names),
        selected_friend: row_of(lists.slots.iter().map(|s| s.guid), social.selected_friend),
        ignores: ignore_rows(lists),
        selected_ignore: row_of(lists.ignores.iter().copied(), social.selected_ignore),
        who: who_rows(social, areas, names),
        // The slot `GetWhoInfo(count + 1)` reads; past the 50th there is none in the array.
        who_next: social
            .who_slots
            .get(social.who_count as usize)
            .map(|e| who_row(e, areas, names)),
        who_total: social.who_total,
        // The chain rides along: `SortWho` re-sorts inside the binding.
        who_sort: social.who_sort.clone(),
    }
}

/// Whether an `SMSG_WHO` answer of `raw` records fires `WHO_LIST_UPDATE` (true) or prints chat
/// lines (false).
fn answer_goes_to_the_frame(to_ui: bool, raw: u32) -> bool {
    to_ui || raw > WHO_CHAT_MAX
}

/// The chat-routed `/who` output: one line per row in the order given, then the count line
/// (`0x5ae0f1`).
fn who_lines(rows: &[WhoInfo], count: u32, get: &dyn Fn(&str) -> Option<String>) -> Vec<Shown> {
    use benilla_ui::strings::{fill, Arg};

    let chat = |text: String| {
        (!text.is_empty()).then(|| Shown::unkeyed(benilla_ui::messages::MsgKind::Chat, text))
    };
    // The composer's name for an id with no row (`0x5ae199`, `0x5ae1d2`, `0x5ae20b`).
    let unknown = get("UNKNOWN").unwrap_or_default();
    let named = |name: &Option<String>| name.clone().unwrap_or_else(|| unknown.clone());
    let mut lines = Vec::with_capacity(rows.len() + 1);
    for row in rows {
        let (race, class, zone) = (named(&row.race), named(&row.class), named(&row.zone));
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
            Arg::S(&race),
            Arg::S(&class),
        ];
        if !row.guild.is_empty() {
            args.push(Arg::S(&row.guild));
        }
        args.push(Arg::S(&zone));
        lines.extend(chat(fill(&template, &args)));
    }
    // `_P1` for any count but one, `GetText`'s plural rule on the count (`0x5ae0f1`), so zero
    // reads "0 players total".
    let count_key = if count == 1 { WHO_KEYS[2] } else { WHO_KEYS[3] };
    if let Some(template) = get(count_key) {
        lines.extend(chat(fill(&template, &[Arg::D(i64::from(count))])));
    }
    lines
}

/// The 50 slots as `GetFriendInfo` (`0x5ad060`) reads them, then the 32 bytes after them, which
/// it reads for index 51 (`0x5ae4b0` bounds at 50 inclusive).
fn friend_rows(
    lists: &Lists,
    areas: Option<&AreaTableCatalog>,
    names: UnitNames<'_>,
) -> Vec<FriendInfo> {
    let mut rows: Vec<FriendInfo> = lists
        .slots
        .iter()
        .map(|slot| friend_row(slot, areas, names))
        .collect();
    // `+0x640`: the pending count's low bytes are the online byte and the flags, the dword at
    // `+0x644` the name, and the first ignore guid and the low half of the second the level, class
    // and area. Nothing writes `+0x644`: the list's allocation is unzeroed (`SMemAlloc` flags 0 at
    // `0x5adc60`) and its constructor `0x5aca10` skips it, so the reference reads whatever the heap
    // held, and benilla holds it null.
    let ignore = |k: usize| lists.ignores.get(k).copied().unwrap_or(0);
    rows.push(friend_row(
        &FriendSlot {
            online: lists.friends_pending & 0xff != 0,
            status: (lists.friends_pending >> 8) as u8,
            name: None,
            guid: 0,
            level: ignore(0) as u32,
            class: (ignore(0) >> 32) as u32,
            area: ignore(1) as u32,
        },
        areas,
        names,
    ));
    rows
}

/// The 25 ignore slots as `GetIgnoreName` (`0x5ad460`) reads them, then the eight bytes after
/// them, which it reads for index 26 (`0x5ae583`): the pending count, then `+0x71c`, which nothing
/// writes. The list's allocation is unzeroed (`SMemAlloc` flags 0 at `0x5adc60`) and its
/// constructor `0x5aca10` skips it, so the reference reads whatever the heap held, and benilla
/// holds 0.
fn ignore_rows(lists: &Lists) -> Vec<IgnoreInfo> {
    let mut rows: Vec<IgnoreInfo> = lists
        .ignores
        .iter()
        .zip(&lists.ignore_names)
        .map(|(&guid, name)| IgnoreInfo {
            name: name.clone().filter(|_| guid != 0),
            listed: guid != 0,
        })
        .collect();
    rows.push(IgnoreInfo {
        name: lists.after_ignores_name.clone(),
        listed: lists.ignores_pending != 0,
    });
    rows
}

/// One slot's answers: its class looked up in `ChrClasses` (`0x5ad0da`-`0x5ad100`) and its area
/// in `AreaTable`, a row with a parent naming the parent (`0x5ad10b`-`0x5ad14f`), whatever the
/// online byte; an id with no row has no name.
fn friend_row(
    slot: &FriendSlot,
    areas: Option<&AreaTableCatalog>,
    names: UnitNames<'_>,
) -> FriendInfo {
    // Both ids are bounded signed against the table's largest (`0x5ad0dd jl`, `0x5ad10e jl`).
    let class = u8::try_from(slot.class)
        .ok()
        .and_then(|id| names.class(id))
        .map(|(display, _)| display.to_string());
    let area = areas.filter(|_| (slot.area as i32) >= 0).and_then(|areas| {
        let row = areas.get(slot.area)?;
        let parent = row.zone_id;
        let shown = ((parent as i32) > 0)
            .then(|| areas.get(parent))
            .flatten()
            .unwrap_or(row);
        Some(shown.name.clone())
    });
    FriendInfo {
        name: slot.name.clone(),
        level: slot.level as i32,
        class,
        area,
        online: slot.online,
        flags: slot.status,
        listed: slot.guid != 0,
    }
}

/// The kept `/who` records, resolved, in the array's order, which the chain sorted.
fn who_rows(
    social: &SocialState,
    areas: Option<&AreaTableCatalog>,
    names: UnitNames<'_>,
) -> Vec<WhoInfo> {
    social.who_slots[..social.who_count as usize]
        .iter()
        .map(|e| who_row(e, areas, names))
        .collect()
}

/// One `/who` row, ids resolved, `None` for an id with no row; the wire carries class before
/// race, the Lua API race first. Each id is bounded whole and signed (`0x5ad75d`, `0x5ad7a0`,
/// `0x5ad7e3`), never by its low byte.
pub(super) fn who_row(
    entry: &WhoEntry,
    areas: Option<&AreaTableCatalog>,
    names: UnitNames<'_>,
) -> WhoInfo {
    let id = |v: u32| u8::try_from(v).ok();
    WhoInfo {
        name: entry.name.clone(),
        guild: entry.guild.clone(),
        level: entry.level,
        race: id(entry.race)
            .and_then(|r| names.race(r))
            .map(|(display, _)| display.to_string()),
        class: id(entry.class)
            .and_then(|c| names.class(c))
            .map(|(display, _)| display.to_string()),
        zone: areas
            .filter(|_| (entry.zone as i32) >= 0)
            .and_then(|a| a.name(entry.zone))
            .map(str::to_string),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::ClientCommand;
    use benilla_protocol::messages::{friend_result, FriendEntry, FriendStatusUpdate, WhoResults};
    use benilla_ui::script::UiScript;

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

    fn apply(social: &mut SocialState, results: WhoResults) {
        social.apply_who(results, None, Default::default());
    }

    fn names(rows: &[WhoInfo]) -> Vec<&str> {
        rows.iter().map(|r| r.name.as_str()).collect()
    }

    /// The reference sorts inside the parse (`0x5ae0e2`), so a list sorted by level stays sorted
    /// when the next `/who` lands.
    #[test]
    fn the_answer_is_presented_in_the_sort_chains_order() {
        let mut social = SocialState::default();
        apply(
            &mut social,
            WhoResults {
                displayed: 3,
                total: 3,
                entries: vec![
                    entry("Galas", 60, ""),
                    entry("erdrin", 12, ""),
                    entry("Bruk", 60, ""),
                ],
            },
        );

        // The seeded chain is zone → level → class → group → name → …: one zone and one class
        // here, so it resolves on level, then name.
        assert_eq!(
            names(&who_rows(&social, None, Default::default())),
            ["erdrin", "Bruk", "Galas"],
            "wire order was Galas, erdrin, Bruk"
        );

        // A Name click: ascending and case-folded, so `erdrin` sorts among the capitals.
        social.who_sort.promote("name");
        social.sort_who(None, Default::default());
        assert_eq!(
            names(&who_rows(&social, None, Default::default())),
            ["Bruk", "erdrin", "Galas"]
        );

        // The same click again reverses.
        social.who_sort.promote("name");
        social.sort_who(None, Default::default());
        assert_eq!(
            names(&who_rows(&social, None, Default::default())),
            ["Galas", "erdrin", "Bruk"]
        );

        // A fresh answer arrives into that same chain.
        apply(
            &mut social,
            WhoResults {
                displayed: 2,
                total: 2,
                entries: vec![entry("Aaa", 1, ""), entry("Zzz", 1, "")],
            },
        );
        assert_eq!(
            names(&who_rows(&social, None, Default::default())),
            ["Zzz", "Aaa"]
        );
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
        apply(
            &mut social,
            WhoResults {
                displayed: 2,
                total: 2,
                entries: vec![entry("Zzz", 60, ""), entry("Aaa", 12, "")],
            },
        );
        // A chain that would order them the other way round, to prove it is not consulted.
        social.who_sort.promote("name");
        social.sort_who(None, Default::default());
        assert_eq!(
            names(&who_rows(&social, None, Default::default())),
            ["Aaa", "Zzz"],
            "the frame's order"
        );

        // The shipped templates: FrameXML never names these keys, so a stub would echo a guess.
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let vm = benilla_ui::script::UiScript::new().expect("VM");
        vm.run(&String::from_utf8_lossy(&src)).expect("runs clean");
        let get = |key: &str| vm.lua().globals().get::<String>(key).ok();
        let lines: Vec<String> = printed_lines(&social, None, Default::default(), &get)
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

    /// The chat composer `0x5ae160` names an id with no row `UNKNOWN`: the race at `0x5ae199`, the
    /// class at `0x5ae1d2`, the zone at `0x5ae20b`.
    #[test]
    fn a_chat_line_names_an_id_with_no_row_unknown() {
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
            crate::ui_unit::TestNames::english().names(),
        );
        let get = |key: &str| match key {
            "WHO_LIST_FORMAT" => Some("|Hplayer:%s|h[%s]|h: Level %d %s %s - %s".to_string()),
            "WHO_NUM_RESULTS" => Some("%d player total".to_string()),
            "UNKNOWN" => Some("Unknown".to_string()),
            _ => None,
        };
        let lines: Vec<String> = who_lines(&[row], 1, &get)
            .iter()
            .map(|s| s.text().to_string())
            .collect();
        assert_eq!(
            lines,
            [
                "|Hplayer:Nobody|h[Nobody]|h: Level 1 Unknown Unknown - Unknown",
                "1 player total"
            ]
        );
    }

    /// The same ids from a race or class wider than a byte: the reference bounds the whole dword
    /// (`0x5ae176`, `0x5ae1af`), so race 0x101 is no race 1.
    #[test]
    fn a_wide_id_names_no_row() {
        let row = who_row(
            &WhoEntry {
                name: "Wide".into(),
                guild: String::new(),
                level: 1,
                class: 0x101,
                race: 0x101,
                zone: 0,
            },
            None,
            crate::ui_unit::TestNames::english().names(),
        );
        let get = |key: &str| match key {
            "WHO_LIST_FORMAT" => Some("%s %s %d %s %s - %s".to_string()),
            "UNKNOWN" => Some("Unknown".to_string()),
            _ => None,
        };
        let lines: Vec<String> = who_lines(&[row], 1, &get)
            .iter()
            .map(|s| s.text().to_string())
            .collect();
        assert_eq!(lines, ["Wide Wide 1 Unknown Unknown - Unknown"]);
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
                race: Some(race.into()),
                class: Some(class.into()),
                zone: Some(zone.into()),
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

    /// A friend's class is the table's word at the install's locale, looked up whatever the online
    /// byte (`0x5ad0da`, the byte first read at `0x5ad15c`); class 0, which the list stores for an
    /// offline friend (`0x5ae415`), has no row.
    #[test]
    fn a_friends_class_is_the_installs_word_online_or_not() {
        let korean = crate::ui_unit::TestNames::korean();
        let slot = |online, class| FriendSlot {
            online,
            guid: 7,
            class,
            ..Default::default()
        };
        let class = |s: &FriendSlot| friend_row(s, None, korean.names()).class;
        assert_eq!(class(&slot(true, 4)).as_deref(), Some("도적"));
        assert_eq!(class(&slot(false, 4)).as_deref(), Some("도적"));
        assert_eq!(class(&slot(false, 0)), None);
        assert_eq!(
            class(&slot(true, 0x104)),
            None,
            "not class 4 by its low byte"
        );
    }

    /// A `/who` row's race and class are the table's words at the install's locale.
    #[test]
    fn a_who_row_names_race_and_class_at_the_installs_locale() {
        let entry = WhoEntry {
            name: "Bob".into(),
            guild: String::new(),
            level: 60,
            class: 8,
            race: 5,
            zone: 0,
        };
        let korean = crate::ui_unit::TestNames::korean();
        let row = who_row(&entry, None, korean.names());
        assert_eq!(
            (row.race.as_deref(), row.class.as_deref()),
            (Some("언데드"), Some("마법사"))
        );
    }

    fn social_app() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<SocialState>()
            .init_resource::<NameCache>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .insert_resource(NetCommands(tx))
            .add_systems(Update, feed_social);
        app.insert_non_send_resource(UiScript::new().unwrap());
        (app, rx)
    }

    fn named(rx: &crossbeam_channel::Receiver<ClientCommand>) -> Vec<u64> {
        crate::query_cache::queries(rx)
            .into_iter()
            .map(|c| match c {
                ClientCommand::NameQuery { guid } => guid,
                other => panic!("not a name query: {other:?}"),
            })
            .collect()
    }

    fn refuse(app: &mut App, guid: u64) {
        app.world_mut()
            .resource_mut::<NameCache>()
            .insert_player(guid, String::new(), None);
    }

    /// Every list function looks its names up with a callback at its packet: the friend list's
    /// (`0x5ae438`), a status's (`0x5acb97`) and the ignore list's (`0x5af59a`). No frame asks a
    /// name, and a refused one is asked again only by the next packet that names its player,
    /// once.
    #[test]
    fn a_refused_social_name_is_asked_at_its_next_packet_and_never_by_a_frame() {
        let (mut app, rx) = social_app();
        app.update();
        let friends = [FriendEntry {
            guid: 7,
            ..Default::default()
        }];
        let packet = |app: &mut App, f: &dyn Fn(&mut SocialState, &dyn super::super::Names)| {
            app.world_mut()
                .resource_scope(|world, mut social: Mut<SocialState>| {
                    let names = super::super::net::CacheNames {
                        names: world.resource::<NameCache>(),
                        net: world.resource::<NetCommands>(),
                    };
                    f(&mut social, &names);
                })
        };
        packet(&mut app, &|social, names| {
            social.apply_friend_list(&friends, names)
        });
        packet(&mut app, &|social, names| {
            social.apply_ignore_list(&[9], names)
        });
        app.update();
        let mut asked = named(&rx);
        asked.sort_unstable();
        assert_eq!(asked, [7, 9], "each name at its list's packet, once");
        for guid in [7, 9] {
            refuse(&mut app, guid);
        }
        for _ in 0..600 {
            app.update();
        }
        assert!(named(&rx).is_empty(), "no frame asks a refusal");

        packet(&mut app, &|social, names| {
            social.apply_friend_status(
                FriendStatusUpdate {
                    result: friend_result::OFFLINE,
                    guid: 7,
                    online: None,
                },
                names,
            );
        });
        app.update();
        assert_eq!(named(&rx), [7], "a status asks its subject");
        packet(&mut app, &|social, names| {
            social.apply_ignore_list(&[9], names)
        });
        for _ in 0..600 {
            app.update();
        }
        assert_eq!(
            named(&rx),
            [9],
            "the ignore list's packet asks its rows, once"
        );
        packet(&mut app, &|social, names| {
            social.apply_friend_list(&friends, names)
        });
        app.update();
        assert!(named(&rx).is_empty(), "7 is still in flight");
    }
}
