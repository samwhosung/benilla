//! The quest chat lines the reference raises from its packet and field handlers: the accept line
//! from the quest-slot watcher (`0x5ddd80`), with an elite quest's tutorial, and the turn-in's
//! completion, experience, money and item lines (`0x5dc400`). Each shows on the questgiver lane
//! where its handler runs ([`QuestGiverLane`]); a line whose quest or item record is not cached
//! waits for it, as the reference's cache callbacks (`0x5ddeb0`, `0x5dc710`, `0x5dc7b0`) print on
//! the record's arrival, inside its answer's handler.

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{QuestGiverEvent, QuestGiverLane};
use crate::field_watch::{FieldNotice, NoticeLane};
use crate::items::Items;
use crate::net::{NetCommands, NetHandlerApp};
use crate::query_cache::Ask;
use crate::ui_action::UiError;
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

/// The lines whose record is in flight, in the order raised, each waiting for its record's
/// answer.
#[derive(Resource, Default)]
pub(crate) struct QuestLines(Vec<QuestLine>);

impl QuestLines {
    #[cfg(test)]
    pub(crate) fn queued(&self) -> &[QuestLine] {
        &self.0
    }
}

/// What raises a line: the caches it looks its record up in, the questgiver lane it shows on and
/// the notice lane an elite quest's tutorial fires on.
pub(crate) struct Raise<'a> {
    pub(crate) quest_log: &'a QuestLog,
    pub(crate) items: &'a Items,
    pub(crate) commands: &'a NetCommands,
    pub(crate) lane: &'a mut QuestGiverLane,
    pub(crate) notices: &'a mut NoticeLane,
    pub(crate) lines: &'a mut QuestLines,
}

impl Raise<'_> {
    /// `line` at its handler: its record looked up with a callback (`0x562a40`, `0x55ba30`), the
    /// line shown now when the record is cached, else held for the record's answer.
    pub(crate) fn line(&mut self, line: QuestLine) {
        let ask = Ask::Event(self.commands);
        match line {
            QuestLine::Accepted(id) | QuestLine::Completed(id) => {
                match self.quest_log.template(id, ask) {
                    Some(t) => show_quest_line(line, t, self.lane, self.notices),
                    None => self.lines.0.push(line),
                }
            }
            QuestLine::Experience(xp) => self.lane.push(QuestGiverEvent::Line(UiError::args(
                "ERR_QUEST_REWARD_EXP_I",
                vec![crate::ui_action::FillArg::D(i64::from(xp))],
            ))),
            QuestLine::Money(copper) => self.lane.push(QuestGiverEvent::RewardMoney(copper)),
            // The name is `0x5d8b00`'s with no random property, the link `0x52adb0`'s.
            QuestLine::Item(entry) => match self.items.template(entry, 0, ask) {
                Some(t) => self.lane.push(QuestGiverEvent::Line(item_line(entry, t))),
                None => {
                    self.items
                        .wait(entry, crate::items::RecordCallback::QuestReward);
                    self.lines.0.push(line);
                }
            },
        }
    }
}

/// A quest-record line shown with its record `t`: the accept line, then tutorial `0x28` for a
/// quest whose `Type` is 1, Elite (`0x5dde6b`-`0x5dde7d`, the callback's `0x5ddf02`-`0x5ddf14`);
/// the completion line unless the quest names a next one in its chain (`0x5dc4d7`).
fn show_quest_line(
    line: QuestLine,
    t: &benilla_protocol::messages::QuestTemplate,
    lane: &mut QuestGiverLane,
    notices: &mut NoticeLane,
) {
    match line {
        QuestLine::Accepted(_) => {
            lane.push(QuestGiverEvent::Line(UiError::s(
                "ERR_QUEST_ACCEPTED_S",
                t.title.clone(),
            )));
            // `QuestInfo.dbc`'s 1 is Elite (`[record+0x10]`, `0x5dde6b`).
            if t.quest_type == 1 {
                notices.write(FieldNotice::Tutorial {
                    id: crate::tutorial::id::ELITE_QUESTS,
                });
            }
        }
        QuestLine::Completed(_) if t.next_quest_in_chain == 0 => lane.push(QuestGiverEvent::Line(
            UiError::s("ERR_QUEST_COMPLETE_S", t.title.clone()),
        )),
        _ => {}
    }
}

/// `ERR_QUEST_REWARD_ITEM_S` with the item's link.
fn item_line(entry: u32, t: &benilla_protocol::ItemInfo) -> UiError {
    UiError::s(
        "ERR_QUEST_REWARD_ITEM_S",
        crate::ui_items::item_link(entry, &t.name, t.quality),
    )
}

impl QuestLines {
    /// The item cache's answer for `entry` runs the first reward line waiting on it (`0x5dc7b0`):
    /// shown with a ready record, dropped with a refusal (`0x5dc7b9`).
    pub(crate) fn item_answered(&mut self, entry: u32, items: &Items, lane: &mut QuestGiverLane) {
        let Some(at) = self.0.iter().position(|&l| l == QuestLine::Item(entry)) else {
            return;
        };
        self.0.remove(at);
        if let Some(t) = items.template(entry, 0, Ask::Read) {
            lane.push(QuestGiverEvent::Line(item_line(entry, t)));
        }
    }
}

pub(super) fn register(app: &mut App) {
    app.init_resource::<QuestLines>()
        .net_handler(SessionEventKind::QuestTemplate, on_template)
        .net_handler(SessionEventKind::Disconnected, on_session_end);
}

/// The quest cache's answer runs the accept and completion lines waiting on it, in the order
/// raised (`0x5ddeb0`, `0x5dc710`); a refusal shows none (`0x5ddec6`, `0x5dc719`).
fn on_template(
    In(ev): In<SessionEvent>,
    mut lines: ResMut<QuestLines>,
    mut lane: ResMut<QuestGiverLane>,
    mut notices: ResMut<NoticeLane>,
) {
    let SessionEvent::QuestTemplate(t) = ev else {
        return;
    };
    let mine = |l: &QuestLine| matches!(l, QuestLine::Accepted(id) | QuestLine::Completed(id) if *id == t.quest_id);
    let (waiting, rest): (Vec<QuestLine>, Vec<QuestLine>) =
        std::mem::take(&mut lines.0).into_iter().partition(mine);
    lines.0 = rest;
    if t.title.is_empty() {
        return;
    }
    for line in waiting {
        show_quest_line(line, &t, &mut lane, &mut notices);
    }
}

/// The waiting lines die with the socket, as the record caches' asks do.
fn on_session_end(In(_): In<SessionEvent>, mut lines: ResMut<QuestLines>) {
    lines.0.clear();
}

/// The turn-in's coin string (`0x5dc511`-`0x5dc5fd`): each paid denomination as `"%d %s"` with
/// its `GOLD`, `SILVER` or `COPPER` global, gold first. The reference's separator test yields
/// `", , "` with a zero middle coin; this joins only the paid ones, with `", "`.
pub(super) fn coin_text(copper: u32, get: &dyn Fn(&str) -> String) -> String {
    // `0x6c6260`'s split by 10000, 100 and 1.
    let (g, s, c) = (copper / 10000, copper / 100 % 100, copper % 100);
    [(g, "GOLD"), (s, "SILVER"), (c, "COPPER")]
        .into_iter()
        .filter(|&(n, _)| n != 0)
        .map(|(n, key)| format!("{n} {}", get(key)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A cached quest template with only its id, title and next step set, for a test.
#[cfg(test)]
pub(crate) fn test_template(
    quest_id: u32,
    title: &str,
    next_quest_in_chain: u32,
) -> benilla_protocol::messages::QuestTemplate {
    use benilla_protocol::messages::{QuestObjective, QuestTemplate};
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
        opposite_rep_faction: 0,
        opposite_rep_value: 0,
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

#[cfg(test)]
mod tests {
    use super::*;

    use crate::net::ClientCommand;

    use super::test_template as template;

    /// The lines' handlers alone, over the caches and the lanes.
    fn app() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<QuestLog>()
            .init_resource::<Items>()
            .init_resource::<QuestGiverLane>()
            .init_resource::<NoticeLane>();
        register(&mut app);
        (app, rx)
    }

    /// Raise `lines` as a handler raises them.
    fn raise(world: &mut World, lines: &[QuestLine]) {
        world.resource_scope(|world, mut waiting: Mut<QuestLines>| {
            world.resource_scope(|world, mut lane: Mut<QuestGiverLane>| {
                world.resource_scope(|world, mut notices: Mut<NoticeLane>| {
                    let mut raise = Raise {
                        quest_log: world.resource::<QuestLog>(),
                        items: world.resource::<Items>(),
                        commands: world.resource::<NetCommands>(),
                        lane: &mut lane,
                        notices: &mut notices,
                        lines: &mut waiting,
                    };
                    for &line in lines {
                        raise.line(line);
                    }
                });
            });
        });
    }

    fn shown(world: &mut World) -> Vec<QuestGiverEvent> {
        world.resource_mut::<QuestGiverLane>().take()
    }

    fn line(key: &'static str, arg: &str) -> QuestGiverEvent {
        QuestGiverEvent::Line(UiError::s(key, arg))
    }

    /// Each line goes on the questgiver lane inside its handler when its record is cached, in the
    /// order raised (`0x5dc400`); a line whose record is in flight asks for it and shows inside the
    /// record's answer (`0x5dc710`, `0x5dc7b0`), the rest not waiting on it.
    #[test]
    fn each_line_shows_as_its_record_lands_and_the_rest_keep_their_order() {
        let (mut app, rx) = app();
        let world = app.world_mut();
        world
            .resource_mut::<QuestLog>()
            .insert_template(template(783, "A Threat Within", 0));
        world
            .resource_mut::<Items>()
            .insert_template(2_580, Some(crate::items::test_template("Linen Cloth")));
        let before = crate::lanes::stamp();
        raise(
            world,
            &[
                QuestLine::Accepted(783),
                QuestLine::Completed(7),
                QuestLine::Experience(450),
                QuestLine::Money(1_025),
                QuestLine::Item(2_580),
                QuestLine::Item(6_529),
            ],
        );
        let stamped = world.resource::<QuestGiverLane>().stamped();
        assert!(stamped.iter().all(|(at, _)| before < *at), "{stamped:?}");
        assert_eq!(
            shown(world),
            [
                line("ERR_QUEST_ACCEPTED_S", "A Threat Within"),
                QuestGiverEvent::Line(UiError::args(
                    "ERR_QUEST_REWARD_EXP_I",
                    vec![crate::ui_action::FillArg::D(450)],
                )),
                QuestGiverEvent::RewardMoney(1_025),
                line(
                    "ERR_QUEST_REWARD_ITEM_S",
                    "|cffffffff|Hitem:2580:0:0:0|h[Linen Cloth]|h|r"
                ),
            ]
        );
        assert_eq!(
            world.resource::<QuestLines>().queued(),
            [QuestLine::Completed(7), QuestLine::Item(6_529)],
            "the uncached quest and item wait"
        );
        let sent: Vec<_> = rx.try_iter().collect();
        assert!(
            sent.iter()
                .any(|c| matches!(c, ClientCommand::QuestQuery { quest: 7 })),
            "the waiting quest's record is asked: {sent:?}"
        );
        assert_eq!(
            world.resource_mut::<Items>().take_callbacks(6_529),
            [crate::items::RecordCallback::QuestReward]
        );

        crate::net::handlers::dispatch(
            world,
            vec![SessionEvent::QuestTemplate(Box::new(template(
                7,
                "Kobold Camp Cleanup",
                0,
            )))],
        );
        assert_eq!(
            shown(world),
            [line("ERR_QUEST_COMPLETE_S", "Kobold Camp Cleanup")]
        );
        world
            .resource_mut::<Items>()
            .insert_template(6_529, Some(crate::items::test_template("Shiny Bauble")));
        world.resource_scope(|world, mut lines: Mut<QuestLines>| {
            let items = world.resource::<Items>();
            let mut lane = QuestGiverLane::default();
            lines.item_answered(6_529, items, &mut lane);
            assert_eq!(
                lane.take(),
                [line(
                    "ERR_QUEST_REWARD_ITEM_S",
                    "|cffffffff|Hitem:6529:0:0:0|h[Shiny Bauble]|h|r"
                )]
            );
        });
        assert!(world.resource::<QuestLines>().queued().is_empty());
    }

    /// `0x5dc4d7`: a quest that names a next quest in its chain completes silently; a refused
    /// record shows nothing at its answer (`0x5dc719`, `0x5dc7b9`).
    #[test]
    fn a_chained_quest_and_a_refused_record_show_nothing() {
        let (mut app, _rx) = app();
        let world = app.world_mut();
        world
            .resource_mut::<QuestLog>()
            .insert_template(template(783, "A Threat Within", 7));
        raise(
            world,
            &[
                QuestLine::Completed(783),
                QuestLine::Completed(8),
                QuestLine::Item(9_999),
            ],
        );
        assert!(shown(world).is_empty());
        crate::net::handlers::dispatch(
            world,
            vec![SessionEvent::QuestTemplate(Box::new(template(8, "", 0)))],
        );
        world.resource_mut::<Items>().insert_template(9_999, None);
        world.resource_scope(|world, mut lines: Mut<QuestLines>| {
            let mut lane = QuestGiverLane::default();
            lines.item_answered(9_999, world.resource::<Items>(), &mut lane);
            assert!(lane.take().is_empty());
        });
        assert!(shown(world).is_empty());
        assert!(world.resource::<QuestLines>().queued().is_empty());
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

    /// The slot watcher's accept line triggers tutorial `0x28`, Elite Quests, when the quest's
    /// record has `Type` 1 (`+0x10`, `0x5dde6b`-`0x5dde7d`), after the line (`0x5dde66`); a record
    /// in flight does both on its landing (`0x5ddefd`, `0x5ddf02`-`0x5ddf14`). Another type
    /// triggers nothing.
    #[test]
    fn an_elite_quests_accept_line_triggers_the_elite_quests_tutorial() {
        let (mut app, _rx) = app();
        let world = app.world_mut();
        let tutorials = |world: &mut World| -> Vec<(crate::lanes::Stamp, FieldNotice)> {
            world.resource_mut::<NoticeLane>().drain_stamped().collect()
        };
        let elite = FieldNotice::Tutorial {
            id: crate::tutorial::id::ELITE_QUESTS,
        };
        let mut t = template(783, "A Threat Within", 0);
        t.quest_type = 1;
        world.resource_mut::<QuestLog>().insert_template(t);
        raise(world, &[QuestLine::Accepted(783)]);
        let lane = world.resource_mut::<QuestGiverLane>().stamped();
        let notices = tutorials(world);
        assert!(
            matches!((lane.as_slice(), notices.as_slice()),
                ([(line_at, _)], [(at, n)]) if line_at < at && *n == elite),
            "the line, then the tutorial: {lane:?} {notices:?}"
        );
        shown(world);

        raise(world, &[QuestLine::Accepted(7)]);
        assert!(tutorials(world).is_empty(), "the record is in flight");
        let mut late = template(7, "Kobold Camp Cleanup", 0);
        late.quest_type = 1;
        crate::net::handlers::dispatch(world, vec![SessionEvent::QuestTemplate(Box::new(late))]);
        assert_eq!(
            tutorials(world)
                .into_iter()
                .map(|(_, n)| n)
                .collect::<Vec<_>>(),
            [elite],
            "on the record's landing"
        );
        shown(world);

        let mut plain = template(8, "Wolves", 0);
        plain.quest_type = 81;
        world.resource_mut::<QuestLog>().insert_template(plain);
        raise(world, &[QuestLine::Accepted(8)]);
        assert!(tutorials(world).is_empty(), "type 81 is no elite quest");
    }
}
