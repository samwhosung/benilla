//! The quest chat lines the reference raises from its packet and field handlers: the accept line
//! from the quest-slot watcher, and the turn-in's completion, experience, money and item lines. A
//! line whose quest or item record is not cached waits for it, as the reference's cache callbacks
//! (`0x5ddeb0`, `0x5dc710`, `0x5dc7b0`) print on the record's arrival.

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use benilla_ui::script::UiScript;

use crate::items::Items;
use crate::net::{NetCommands, NetHandlerApp};
use crate::ui_action::{show_messages, ui_error_text, FillArg, MessageSink, Shown, UiError};
use crate::ui_quest_log::QuestLog;

/// One quest chat line, raised in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuestLine {
    /// `ERR_QUEST_ACCEPTED_S` (`0x89`) with the quest's title (`0x5dde66`).
    Accepted(u32),
    /// `ERR_QUEST_COMPLETE_S` (`0x8a`) with the title, unless the quest names a next quest in its
    /// chain (`record+0x24`, `0x5dc4d7`).
    Completed(u32),
    /// `ERR_QUEST_REWARD_EXP_I` (`0x95`, `0x5dc4f9`).
    Experience(u32),
    /// `ERR_QUEST_REWARD_MONEY_S` (`0x97`, `0x5dc609`) with the coin string.
    Money(u32),
    /// `ERR_QUEST_REWARD_ITEM_S` (`0x96`, `0x5dc822`) with the item's link.
    Item(u32),
}

/// The lines raised and not yet shown.
#[derive(Resource, Default)]
pub(crate) struct QuestLines(Vec<QuestLine>);

impl QuestLines {
    pub(crate) fn push(&mut self, line: QuestLine) {
        self.0.push(line);
    }

    #[cfg(test)]
    pub(crate) fn queued(&self) -> &[QuestLine] {
        &self.0
    }
}

/// What a line resolves to this frame.
enum Resolved {
    Show(UiError),
    /// Its record is still in flight.
    Wait,
    /// The server does not know the quest or item, or the line has nothing to say.
    Drop,
}

pub(super) fn register(app: &mut App) {
    app.init_resource::<QuestLines>()
        .net_handler(SessionEventKind::Disconnected, on_session_end)
        .add_systems(
            Update,
            feed_quest_lines
                .in_set(crate::ui_unit::UnitFeed)
                .after(crate::ui_quest_log::feed_quest_log),
        );
}

/// The waiting lines die with the socket, as the record caches do.
fn on_session_end(In(_): In<SessionEvent>, mut lines: ResMut<QuestLines>) {
    lines.0.clear();
}

/// Show every line whose record is here, in the order raised; the rest wait.
fn feed_quest_lines(
    script: Option<NonSendMut<UiScript>>,
    mut lines: ResMut<QuestLines>,
    quest_log: Res<QuestLog>,
    items: Res<Items>,
    commands: Res<NetCommands>,
    mut sink: MessageSink,
) {
    let Some(mut script) = script else {
        return;
    };
    if lines.0.is_empty() {
        return;
    }
    let get = |key: &str| benilla_ui::strings::global(script.lua(), key);
    let mut shown = Vec::new();
    lines.0.retain(
        |&line| match resolve(line, &quest_log, &items, &commands, &get) {
            Resolved::Show(msg) => {
                if let Some(text) = ui_error_text(&msg, &get) {
                    shown.push(Shown::keyed(msg.key, text));
                }
                false
            }
            Resolved::Wait => true,
            Resolved::Drop => false,
        },
    );
    show_messages(&mut script, &mut sink, "ui_quest", shown);
}

fn resolve(
    line: QuestLine,
    quest_log: &QuestLog,
    items: &Items,
    commands: &NetCommands,
    get: &dyn Fn(&str) -> Option<String>,
) -> Resolved {
    match line {
        QuestLine::Accepted(id) => match quest(id, quest_log, commands) {
            Ok(t) => Resolved::Show(UiError::s("ERR_QUEST_ACCEPTED_S", t.title.clone())),
            Err(r) => r,
        },
        QuestLine::Completed(id) => match quest(id, quest_log, commands) {
            Ok(t) if t.next_quest_in_chain == 0 => {
                Resolved::Show(UiError::s("ERR_QUEST_COMPLETE_S", t.title.clone()))
            }
            Ok(_) => Resolved::Drop,
            Err(r) => r,
        },
        QuestLine::Experience(xp) => Resolved::Show(UiError::args(
            "ERR_QUEST_REWARD_EXP_I",
            vec![FillArg::D(i64::from(xp))],
        )),
        QuestLine::Money(copper) => Resolved::Show(UiError::s(
            "ERR_QUEST_REWARD_MONEY_S",
            coin_text(copper, &|k| get(k).unwrap_or_default()),
        )),
        // The name is `0x5d8b00`'s with no random property, the link `0x52adb0`'s.
        QuestLine::Item(entry) => match items.template(entry, 0, commands) {
            Some(t) => Resolved::Show(UiError::s(
                "ERR_QUEST_REWARD_ITEM_S",
                crate::ui_items::item_link(entry, &t.name, t.quality),
            )),
            None if items.template_answered_unknown(entry) => Resolved::Drop,
            None => Resolved::Wait,
        },
    }
}

/// The quest's record, or what its line does without one.
fn quest<'a>(
    id: u32,
    quest_log: &'a QuestLog,
    commands: &NetCommands,
) -> Result<&'a benilla_protocol::messages::QuestTemplate, Resolved> {
    match quest_log.template(id, commands) {
        Some(t) => Ok(t),
        None if quest_log.template_answered_unknown(id) => Err(Resolved::Drop),
        None => Err(Resolved::Wait),
    }
}

/// The turn-in's coin string (`0x5dc511`-`0x5dc5fd`): each paid denomination as `"%d %s"` with
/// its `GOLD`, `SILVER` or `COPPER` global, gold first. The reference's separator test yields
/// `", , "` with a zero middle coin; this joins only the paid ones, with `", "`.
fn coin_text(copper: u32, get: &dyn Fn(&str) -> String) -> String {
    // `0x6c6260`'s split by 10000, 100 and 1.
    let (g, s, c) = (copper / 10000, copper / 100 % 100, copper % 100);
    [(g, "GOLD"), (s, "SILVER"), (c, "COPPER")]
        .into_iter()
        .filter(|&(n, _)| n != 0)
        .map(|(n, key)| format!("{n} {}", get(key)))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::{QuestObjective, QuestTemplate};

    use crate::net::ClientCommand;

    fn template(quest_id: u32, title: &str, next_quest_in_chain: u32) -> QuestTemplate {
        let objective = || QuestObjective {
            creature_or_go: 0,
            required_count: 0,
            item_id: 0,
            item_count: 0,
            text: String::new(),
        };
        QuestTemplate {
            quest_id,
            method: 2,
            level: 1,
            zone_or_sort: 12,
            quest_type: 0,
            rep_objective_faction: 0,
            rep_objective_value: 0,
            next_quest_in_chain,
            money: 0,
            money_max_level: 0,
            reward_spell: 0,
            src_item_id: 0,
            flags: 0,
            rewards: [(0, 0); 4],
            choices: [(0, 0); 6],
            point_map_id: 0,
            point_x: 0.0,
            point_y: 0.0,
            point_opt: 0,
            title: title.into(),
            objectives_text: String::new(),
            details: String::new(),
            end_text: String::new(),
            objectives: std::array::from_fn(|_| objective()),
        }
    }

    /// The feed alone, over a VM holding the stock enUS templates.
    fn app() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<QuestLines>()
            .init_resource::<QuestLog>()
            .init_resource::<Items>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .add_systems(Update, feed_quest_lines);
        let script = UiScript::new().unwrap();
        script
            .run(
                r#"
                ERR_QUEST_ACCEPTED_S = "Quest accepted: %s"
                ERR_QUEST_COMPLETE_S = "%s completed."
                ERR_QUEST_REWARD_EXP_I = "Experience gained: %d."
                ERR_QUEST_REWARD_MONEY_S = "Received %s."
                ERR_QUEST_REWARD_ITEM_S = "Received item: %s."
                GOLD = "Gold"; SILVER = "Silver"; COPPER = "Copper"
                "#,
            )
            .unwrap();
        app.insert_non_send_resource(script);
        (app, rx)
    }

    fn chat(app: &App) -> Vec<String> {
        app.world()
            .resource::<crate::ui_chat::ChatLog>()
            .pending_lines()
    }

    #[test]
    fn each_line_shows_as_its_record_lands_and_the_rest_keep_their_order() {
        let (mut app, rx) = app();
        {
            let world = app.world_mut();
            world
                .resource_mut::<QuestLog>()
                .insert_template(template(783, "A Threat Within", 0));
            world
                .resource_mut::<Items>()
                .insert_template(2_580, Some(crate::items::test_template("Linen Cloth")));
            let mut lines = world.resource_mut::<QuestLines>();
            for line in [
                QuestLine::Accepted(783),
                QuestLine::Completed(7),
                QuestLine::Experience(450),
                QuestLine::Money(1_025),
                QuestLine::Item(2_580),
                QuestLine::Item(6_529),
            ] {
                lines.push(line);
            }
        }
        app.update();
        assert_eq!(
            chat(&app),
            [
                "Quest accepted: A Threat Within",
                "Experience gained: 450.",
                "Received 10 Silver, 25 Copper.",
                "Received item: |cffffffff|Hitem:2580:0:0:0|h[Linen Cloth]|h|r.",
            ]
        );
        assert_eq!(
            app.world().resource::<QuestLines>().queued(),
            [QuestLine::Completed(7), QuestLine::Item(6_529)],
            "the uncached quest and item wait"
        );
        let sent: Vec<_> = rx.try_iter().collect();
        assert!(
            sent.iter()
                .any(|c| matches!(c, ClientCommand::QuestQuery { quest: 7 })),
            "the waiting quest's record is asked: {sent:?}"
        );
        assert!(
            app.world()
                .resource::<crate::sound::MessageSounds>()
                .queued()
                .iter()
                .any(|r| r.key == "ERR_QUEST_ACCEPTED_S"),
            "the accept line's row carries its sound"
        );

        {
            let world = app.world_mut();
            world
                .resource_mut::<QuestLog>()
                .insert_template(template(7, "Kobold Camp Cleanup", 0));
            world
                .resource_mut::<Items>()
                .insert_template(6_529, Some(crate::items::test_template("Shiny Bauble")));
        }
        app.update();
        assert_eq!(
            chat(&app)[4..],
            [
                "Kobold Camp Cleanup completed.",
                "Received item: |cffffffff|Hitem:6529:0:0:0|h[Shiny Bauble]|h|r.",
            ]
        );
        assert!(app.world().resource::<QuestLines>().queued().is_empty());
    }

    /// `0x5dc4d7`: a quest that names a next quest in its chain completes silently.
    #[test]
    fn a_chained_quest_and_an_unknown_record_show_nothing() {
        let (mut app, _rx) = app();
        {
            let world = app.world_mut();
            world
                .resource_mut::<QuestLog>()
                .insert_template(template(783, "A Threat Within", 7));
            world.resource_mut::<Items>().insert_template(9_999, None);
            let mut lines = world.resource_mut::<QuestLines>();
            lines.push(QuestLine::Completed(783));
            lines.push(QuestLine::Item(9_999));
        }
        app.update();
        assert!(chat(&app).is_empty());
        assert!(app.world().resource::<QuestLines>().queued().is_empty());
    }

    fn words(key: &str) -> String {
        match key {
            "GOLD" => "Gold",
            "SILVER" => "Silver",
            "COPPER" => "Copper",
            _ => "",
        }
        .into()
    }

    #[test]
    fn the_coin_string_joins_only_the_paid_denominations() {
        assert_eq!(coin_text(5, &words), "5 Copper");
        assert_eq!(coin_text(250, &words), "2 Silver, 50 Copper");
        assert_eq!(coin_text(10_200, &words), "1 Gold, 2 Silver");
        assert_eq!(coin_text(12_345, &words), "1 Gold, 23 Silver, 45 Copper");
        assert_eq!(coin_text(30_000, &words), "3 Gold");
        // A zero middle coin leaves no empty place between its neighbours.
        assert_eq!(coin_text(10_005, &words), "1 Gold, 5 Copper");
    }
}
