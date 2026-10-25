//! What the quest log's getters ask the lent host as they run ([`super::live::ask`]), each as of
//! the place the getter runs, as the reference's read the live object and caches at the call: the
//! player's quest slots (`0x4dfa41`, `0x4e01df`), the coinage (`0x4df70b`), the held count of an
//! item (`0x622130`), a faction's standing (`0x4d6370`) and the clock (`0x4deef0`'s offset, the
//! local `time`, `0x429580`). Each falls back to the VM's copy with no host to ask.

use mlua::Lua;

use super::live::{ask, LiveQuery};
use super::{Model, QuestSlotView, QUEST_LOG_SLOTS};

/// The player's twenty quest slots, `PLAYER_QUEST_LOG_1_1` on (`0x4de73e`); declined while no
/// player is held.
#[derive(Clone, Copy, Debug)]
pub struct QuestSlots;

impl LiveQuery for QuestSlots {
    type Answer = [QuestSlotView; QUEST_LOG_SLOTS];
}

/// How many of an item the player holds, as a quest objective counts them: bags, bank and keyring
/// (`0x622130` with mask 8).
#[derive(Clone, Copy, Debug)]
pub struct QuestHeld(pub u32);

impl LiveQuery for QuestHeld {
    type Answer = u32;
}

/// A faction's standing as the turn-in test and the reputation line read it: the base for the
/// player's race and class plus the wire's (`0x4d6370`), 0 for a faction with no reputation index.
#[derive(Clone, Copy, Debug)]
pub struct QuestStanding(pub u32);

impl LiveQuery for QuestStanding {
    type Answer = i32;
}

/// The player's coinage (`PLAYER_FIELD_COINAGE`, `+0xf70`), which a quest that asks money tests.
#[derive(Clone, Copy, Debug)]
pub struct QuestCoinage;

impl LiveQuery for QuestCoinage {
    type Answer = u64;
}

/// The clock a timed quest counts down by.
#[derive(Clone, Copy, Debug)]
pub struct QuestClock;

/// A [`QuestClock`]'s answer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QuestClockRead {
    /// The server's unix seconds now, `None` before the first `SMSG_QUERY_TIME_RESPONSE`.
    pub server: Option<f64>,
    /// Whether the last sync found the local and server clocks on one second, which the
    /// reference stores as an offset of 1 (`0x4def00`, `0x4def02`).
    pub zero: bool,
    /// The local clock in unix seconds (`0x429580`), `None` in a VM nothing has set it in.
    pub local: Option<f64>,
}

impl LiveQuery for QuestClock {
    type Answer = QuestClockRead;
}

/// The quest slots now: the host's, else the VM's copy.
pub(super) fn slots(lua: &Lua, model: &Model) -> [QuestSlotView; QUEST_LOG_SLOTS] {
    ask(lua, &QuestSlots).unwrap_or(model.quest_store.slots)
}

/// Slot `slot` now (`None` past the twenty).
pub(super) fn slot(lua: &Lua, model: &Model, slot: u8) -> Option<QuestSlotView> {
    slots(lua, model).get(usize::from(slot)).copied()
}

/// The held count of `item_id` now. The copy is test-only: every production entry that runs Lua
/// (the tick, the lanes, the load) lends the host, and a harness without one has the copy.
pub(super) fn held(lua: &Lua, model: &Model, item_id: u32) -> u32 {
    ask(lua, &QuestHeld(item_id))
        .unwrap_or_else(|| model.quest_store.held.get(&item_id).copied().unwrap_or(0))
}

/// `faction`'s standing now, 0 for one the player has no slot for.
pub(super) fn standing(lua: &Lua, model: &Model, faction: u32) -> i32 {
    ask(lua, &QuestStanding(faction))
        .unwrap_or_else(|| model.faction_standings.get(&faction).copied().unwrap_or(0))
}

/// The coinage now.
pub(super) fn coinage(lua: &Lua, model: &Model) -> u64 {
    ask(lua, &QuestCoinage).unwrap_or(model.money)
}

/// The clock now: the host's, else the VM's copy, which only a test sets (production lends the
/// host). The host's clock is the dispatch's, not each packet's stamp (`0x4deefa`): the flip settles it.
pub(super) fn clock(lua: &Lua, model: &Model) -> QuestClockRead {
    ask(lua, &QuestClock).unwrap_or(QuestClockRead {
        server: model.server_unix_time,
        zero: model.clock_sync_zero,
        local: model.local_unix_time,
    })
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::collections::HashMap;

    use super::*;
    use crate::script::live::LiveAsk;
    use crate::script::quest_log::fixture::{collect, kill, record, seat, slot};
    use crate::script::{CallHost, CallTail, ItemTemplateView, ScriptCall, UiScript};

    /// A host that holds the world's quest state and answers the getters' questions from it,
    /// each as of the ask.
    #[derive(Default)]
    struct World {
        slots: Option<[QuestSlotView; QUEST_LOG_SLOTS]>,
        held: HashMap<u32, u32>,
        standings: HashMap<u32, i32>,
        coinage: Option<u64>,
        clock: Option<QuestClockRead>,
        asked: Cell<usize>,
    }

    impl CallHost for World {
        fn apply(&mut self, _: &mut UiScript, _: ScriptCall) -> Option<CallTail> {
            None
        }

        fn read_live(&mut self, ask: &mut LiveAsk<'_>) -> bool {
            self.asked.set(self.asked.get() + 1);
            ask.answer(|_: &QuestSlots| self.slots)
                || ask.answer(|q: &QuestHeld| Some(self.held.get(&q.0).copied().unwrap_or(0)))
                || ask.answer(|q: &QuestStanding| {
                    Some(self.standings.get(&q.0).copied().unwrap_or(0))
                })
                || ask.answer(|_: &QuestCoinage| self.coinage)
                || ask.answer(|_: &QuestClock| self.clock)
        }
    }

    fn slots_of(slot: QuestSlotView) -> [QuestSlotView; QUEST_LOG_SLOTS] {
        let mut slots = [QuestSlotView::default(); QUEST_LOG_SLOTS];
        slots[0] = slot;
        slots
    }

    fn complete(s: &UiScript) -> Option<i64> {
        s.eval::<Option<i64>>("local _, _, _, _, _, c = GetQuestLogTitle(2) return c")
            .unwrap()
    }

    /// An objective's counter is read off the slot the host holds at the call (`0x4e0302`,
    /// `0x4dfa41`), not the copy the last rebuild took: the line, its finished flag and the
    /// quest's `isComplete` follow the world, and with no host the copy answers.
    #[test]
    fn the_lines_and_the_turn_in_read_the_slot_the_host_holds() {
        let mut hunt = record("Hunt", 1, 12, "Elwynn Forest");
        hunt.objectives[0] = kill(40, 3);
        let mut s = UiScript::new().unwrap();
        s.set_quest_creature_name(40, "Wolf".into());
        seat(&mut s, &[(50, hunt, slot(50, 1, 0))]);
        let line = |s: &UiScript| {
            s.eval::<(String, Option<i64>)>(
                "local t, _, f = GetQuestLogLeaderBoard(1, 2) return t, f",
            )
            .unwrap()
        };
        assert_eq!(line(&s), ("Wolf slain: 1/3".to_string(), None));
        let mut host = World {
            slots: Some(slots_of(slot(50, 3, 0))),
            ..Default::default()
        };
        s.hosted(&mut host, |s| {
            assert_eq!(line(s), ("Wolf slain: 3/3".to_string(), Some(1)));
            assert_eq!(
                complete(s),
                Some(1),
                "the turn-in test over the world's counter"
            );
        });
        assert!(host.asked.get() > 0);
        assert_eq!(complete(&s), None, "with no host the copy answers");
        assert_eq!(line(&s), ("Wolf slain: 1/3".to_string(), None));
    }

    /// A failed bit the world holds shows at once (`0x4dfa61`), and the timer reads the host's
    /// deadline and clock (`0x4e0905`).
    #[test]
    fn the_failed_bit_and_the_timers_read_the_slot_and_the_clock_the_host_holds() {
        let mut hunt = record("Hunt", 1, 12, "Elwynn Forest");
        hunt.objectives[0] = kill(40, 3);
        let mut s = UiScript::new().unwrap();
        seat(&mut s, &[(50, hunt, slot(50, 0, 0))]);
        let mut timed = slot(50, 0, 0);
        timed.timer = 1100;
        let mut host = World {
            slots: Some(slots_of(timed)),
            clock: Some(QuestClockRead {
                server: Some(1000.5),
                zero: false,
                local: Some(1.0),
            }),
            ..Default::default()
        };
        s.hosted(&mut host, |s| {
            assert_eq!(
                s.eval::<Option<i64>>("return GetQuestTimers()").unwrap(),
                Some(99),
                "1100 - 1000 - 1"
            );
            s.run("SelectQuestLogEntry(2)").unwrap();
            assert_eq!(
                s.eval::<Option<i64>>("return GetQuestLogTimeLeft()")
                    .unwrap(),
                Some(99),
                "the selected quest's slot, synced by the host's clock"
            );
        });
        assert_eq!(
            s.eval::<Option<i64>>("return GetQuestTimers()").unwrap(),
            None,
            "an untimed copy has no timer"
        );
        let mut failed = timed;
        failed.state = 0x02;
        host.slots = Some(slots_of(failed));
        s.hosted(&mut host, |s| {
            assert_eq!(complete(s), Some(-1));
        });
    }

    /// The held count of an item objective is the host's at the call (`0x4e0579`, capped at the
    /// need), and so is the turn-in test's (`0x4df778`).
    #[test]
    fn an_item_line_reads_the_count_the_host_holds() {
        let mut r = record("Pelts", 1, 12, "Elwynn Forest");
        r.objectives[0] = collect(900, 5);
        let mut s = UiScript::new().unwrap();
        s.set_item_template(
            900,
            ItemTemplateView {
                name: "Wolf Pelt".into(),
                ..Default::default()
            },
        );
        seat(&mut s, &[(50, r, slot(50, 0, 0))]);
        s.set_quest_item_counts([(900, 1)].into());
        let line = |s: &UiScript| {
            s.eval::<String>("return (GetQuestLogLeaderBoard(1, 2))")
                .unwrap()
        };
        let mut host = World {
            held: [(900, 7)].into(),
            ..Default::default()
        };
        s.hosted(&mut host, |s| {
            assert_eq!(line(s), "Wolf Pelt: 5/5");
            assert_eq!(complete(s), Some(1));
        });
        assert_eq!(line(&s), "Wolf Pelt: 1/5");
    }

    /// The reputation line and the turn-in test read the host's standing (`0x4d6370`), the coinage
    /// a negative money asks the host's (`0x4df70b`).
    #[test]
    fn the_standing_and_the_coinage_are_the_hosts() {
        let mut r = record("Favor", 1, 12, "Elwynn Forest");
        r.rep_min = (72, 3000);
        r.rep_faction_name = Some("Stormwind".into());
        r.required_money = 100;
        let mut s = UiScript::new().unwrap();
        seat(&mut s, &[(50, r, slot(50, 0, 0))]);
        s.set_faction_standings([(72, 0)].into());
        s.set_money(0);
        let mut host = World {
            standings: [(72, 3500)].into(),
            coinage: Some(150),
            ..Default::default()
        };
        s.hosted(&mut host, |s| {
            assert_eq!(complete(s), Some(1));
            assert_eq!(
                s.eval::<Option<i64>>("local _, _, f = GetQuestLogLeaderBoard(1, 2) return f")
                    .unwrap(),
                Some(1),
                "the reputation line is finished at the host's standing"
            );
        });
        assert_eq!(complete(&s), None, "the copy holds neither");
        host.coinage = Some(99);
        s.hosted(&mut host, |s| {
            assert_eq!(complete(s), None, "short of the host's coinage");
        });
    }

    /// The list is built from the slots the host holds where the rebuild runs (`0x4de63c`), the
    /// copy the call carried or an earlier rebuild left ignored.
    #[test]
    fn the_rebuild_reads_the_slots_the_host_holds() {
        let a = record("Hunt", 1, 12, "Elwynn Forest");
        let b = record("Hunt Again", 2, 12, "Elwynn Forest");
        let mut s = UiScript::new().unwrap();
        seat(&mut s, &[(50, a, slot(50, 0, 0))]);
        s.seed_quest_record(51, b);
        let count = |s: &UiScript| {
            s.eval::<i64>("local _, n = GetNumQuestLogEntries() return n")
                .unwrap()
        };
        assert_eq!(count(&s), 1);
        let mut slots = slots_of(slot(50, 0, 0));
        slots[1] = slot(51, 0, 0);
        let mut host = World {
            slots: Some(slots),
            ..Default::default()
        };
        s.hosted(&mut host, |s| {
            s.quest_log_changed();
        });
        assert_eq!(count(&s), 2, "the quest the world holds got its row");
    }
}
