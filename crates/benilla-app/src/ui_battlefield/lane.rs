//! The battleground lane: the events the battleground packets' handlers raise, each fired at its
//! packet's place among the other lanes' items ([`crate::lanes`]). In the reference each handler
//! writes the module's globals and signals inside itself: the list (`0x4aa6c0`) its
//! `BATTLEFIELDS_SHOW` (`0x4aa7d1`), the status (`0x4aa850`) its `UPDATE_BATTLEFIELD_SCORE`
//! (`0x4aaa5a`) and `UPDATE_BATTLEFIELD_STATUS` (`0x4aab05`), the log (`0x4aab30`) its rebuild's
//! `UPDATE_BATTLEFIELD_SCORE` (`0x4aa308`), the verdict (`0x4aacc0`) and the joined and left
//! notices (`0x4aaef0`, `0x4aafe0`) their lines, and a name answer the callbacks waiting on it
//! (`0x4a9ef0`, `0x4ac190`). A handler queues its event here, stamped where it runs, and the
//! getters its listeners call read the live state through the host.

use std::collections::VecDeque;

use bevy::prelude::*;

use benilla_assets::MapCatalogRes;
use benilla_ui::script::UiScript;

use super::{Battlefield, BattlefieldLeash, GROUP_JOIN_DESERTERS};
use crate::lanes::{LaneApp, Stamp, Until};
use crate::names::NameCache;
use crate::player::Player;
use crate::ui_action::MessageSink;
use crate::ui_battlefield_positions::BattlefieldPositions;
use crate::ui_battlefield_score::{BattlefieldScoreboard, BoardView};

/// What an item signals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum BattlefieldEvent {
    /// `BATTLEFIELDS_SHOW` (`0x4aa7d1`); the leash's anchor is stored after it (`0x4aa7ff`).
    Show,
    /// `UPDATE_BATTLEFIELD_STATUS` (`0x4aab05`).
    Status,
    /// `UPDATE_BATTLEFIELD_SCORE`, a rebuild's (`0x4aa308`).
    Score,
    /// `WORLD_MAP_NAME_UPDATE`, one spent callback's (`0x4ac195`).
    MapName,
    /// `SMSG_GROUP_JOINED_BATTLEGROUND`'s line (`0x4aacc0`): 439 for the deserters, 440 with a
    /// listed map's name, else 441.
    Verdict(u32),
    /// A joined (`ERR_BG_PLAYER_JOINED_SS`, `0x4aaef0`) or left (`ERR_BG_PLAYER_LEFT_S`,
    /// `0x4aafe0`) notice, with the name.
    Player { name: String, joined: bool },
}

/// The battleground lane's queue.
#[derive(Resource, Default)]
pub(crate) struct BattlefieldLane {
    events: VecDeque<(Stamp, BattlefieldEvent)>,
}

impl BattlefieldLane {
    /// Queue `event` stamped now, behind everything stamped before it.
    pub(crate) fn push(&mut self, event: BattlefieldEvent) {
        let stamp = crate::lanes::stamp();
        let at = self.events.partition_point(|(s, _)| *s < stamp);
        self.events.insert(at, (stamp, event));
    }

    fn head(&self) -> Option<Stamp> {
        self.events.front().map(|(s, _)| *s)
    }

    /// What the queued items signal, in order, taking them.
    #[cfg(test)]
    pub(crate) fn take(&mut self) -> Vec<BattlefieldEvent> {
        self.events.drain(..).map(|(_, e)| e).collect()
    }
}

/// The lane, and its drop while no in-game interface is up to hear it: a signal is for the
/// interface that was up.
pub(super) fn add_battlefield_lane(app: &mut App) {
    app.init_resource::<BattlefieldLane>()
        .add_lane(
            "battlefield",
            |world| world.get_resource::<BattlefieldLane>()?.head(),
            fire_battlefield,
            None,
        )
        .add_systems(
            Update,
            drop_battlefield_events
                .in_set(crate::ui_script::UiFeed)
                .run_if(not(crate::ui_script::ingame_ui_up)),
        );
}

fn drop_battlefield_events(mut lane: ResMut<BattlefieldLane>) {
    lane.events.clear();
}

/// The lane's firing: each item the drain admits, signalled with the host lent. It stops after an
/// item whose firing stamped anything, so what that pushed fires next.
fn fire_battlefield(
    In(until): In<Until>,
    mut vm: crate::script_calls::HostedVm,
    mut lane: ResMut<BattlefieldLane>,
    mut leash: ResMut<BattlefieldLeash>,
    player: Res<Player>,
    maps: Option<Res<MapCatalogRes>>,
    mut sink: MessageSink,
) {
    if !vm.run(|script| script.is_some()) {
        return;
    }
    while let Some(at) = lane.head().filter(|&s| until.admits(s)) {
        let Some((_, event)) = lane.events.pop_front() else {
            return;
        };
        let show = event == BattlefieldEvent::Show;
        let ((), moved) = crate::lanes::firing(at, || {
            vm.run(|script| {
                if let Some(script) = script {
                    fire(script, &mut sink, maps.as_deref(), event);
                }
            })
        });
        // The anchor is the player's position as the handler returns from its signal (`0x4aa7ff`).
        if show {
            leash.0 = Some(player.pos);
        }
        if moved {
            return;
        }
    }
}

/// One item's signal.
fn fire(
    script: &mut UiScript,
    sink: &mut MessageSink,
    maps: Option<&MapCatalogRes>,
    event: BattlefieldEvent,
) {
    use crate::ui_action::{keyed_line, keyed_line_s, show_now};
    let line = match event {
        BattlefieldEvent::Show => return script.fire_event("BATTLEFIELDS_SHOW", vec![]),
        BattlefieldEvent::Status => return script.fire_event("UPDATE_BATTLEFIELD_STATUS", vec![]),
        BattlefieldEvent::Score => return script.fire_event("UPDATE_BATTLEFIELD_SCORE", vec![]),
        BattlefieldEvent::MapName => return script.fire_event("WORLD_MAP_NAME_UPDATE", vec![]),
        BattlefieldEvent::Verdict(result) => {
            if result == GROUP_JOIN_DESERTERS {
                keyed_line(script, "ERR_GROUP_JOIN_BATTLEGROUND_DESERTERS")
            } else if let Some(name) = maps.and_then(|m| m.0.name(result)) {
                keyed_line_s(script, "ERR_GROUP_JOIN_BATTLEGROUND_S", &[name])
            } else {
                keyed_line(script, "ERR_GROUP_JOIN_BATTLEGROUND_FAIL")
            }
        }
        BattlefieldEvent::Player { name, joined: true } => {
            keyed_line_s(script, "ERR_BG_PLAYER_JOINED_SS", &[&name, &name])
        }
        BattlefieldEvent::Player {
            name,
            joined: false,
        } => keyed_line_s(script, "ERR_BG_PLAYER_LEFT_S", &[&name]),
    };
    // Shown through `0x496720`, inside the handler that adds it.
    if let Some(line) = line {
        show_now(script, sink, "ui_battlefield", line);
    }
}

/// The battleground callbacks a player-name answer runs, a record or a refusal, after the cache
/// files it (`0x55f426`): the score's count-down (`0x4a9ef0`), each map callback's
/// `WORLD_MAP_NAME_UPDATE` (`0x4ac190`) and each notice waiting on the name, which a refusal spends
/// silently (`0x4aaf2f`).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct BattlefieldNameAnswers<'w> {
    /// Each absent in a harness without the battleground windows.
    lane: Option<ResMut<'w, BattlefieldLane>>,
    list: Option<ResMut<'w, Battlefield>>,
    board: Option<ResMut<'w, BattlefieldScoreboard>>,
    positions: Option<ResMut<'w, BattlefieldPositions>>,
    tables: crate::ui_unit::NameTables<'w>,
}

impl BattlefieldNameAnswers<'_> {
    /// `guid`'s answer just filed in `names`.
    pub(crate) fn answered(&mut self, guid: u64, names: &NameCache) {
        let Some(lane) = self.lane.as_deref_mut() else {
            return;
        };
        let name = names.resolve_player(guid, crate::query_cache::Ask::Read);
        if let Some(list) = self.list.as_deref_mut() {
            for joined in list.take_waits(guid) {
                if let Some(name) = name {
                    lane.push(BattlefieldEvent::Player {
                        name: name.to_string(),
                        joined,
                    });
                }
            }
        }
        if let Some(board) = self.board.as_deref_mut() {
            let at = BoardView {
                names,
                tables: self.tables.get(),
            };
            if board.name_answered(guid, at) {
                lane.push(BattlefieldEvent::Score);
            }
        }
        if let Some(positions) = self.positions.as_deref_mut() {
            for _ in 0..positions.name_answered(guid) {
                lane.push(BattlefieldEvent::MapName);
            }
        }
    }
}
