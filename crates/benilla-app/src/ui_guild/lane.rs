//! The guild lane: the events the guild packets' handlers raise, each fired at its packet's
//! place among the other lanes' items ([`crate::lanes`]), with the state that packet left. In the
//! reference each handler writes the roster module's cells and signals inside itself: the roster
//! message `0x4d0ad0` ends in `GUILD_ROSTER_UPDATE` (`0x4d1160`), the guild event `0x5e7180`
//! signals `GUILD_MOTD` (`0x5e728d`) and then `0x4d1160(1)` (`0x5e74c4`), the command result
//! `0x5e7520` calls `0x4d1160(1)` for two results, and the invitation `0x5e6f20` signals
//! `GUILD_INVITE_REQUEST` (`0x48f45c`). So a listener's getters read the cells its own packet
//! left. A handler queues its event here with the version of the state it wrote
//! ([`GuildState::raise`]), and a write that signals nothing queues its version alone
//! ([`GuildState::settle`]); the lane fires an item with its version as the state the getters
//! read ([`GuildState::seen`]), and the drain's end leaves the live state.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};

use benilla_protocol::messages::GuildInfo;
use benilla_ui::script::{ScriptValue, UiScript};
use bevy::prelude::*;

use super::view::GuildView;
use super::{GuildState, RosterUpdate};
use crate::lanes::{LaneApp, Stamp, Until};
use crate::script_calls::VmWith;
use crate::ui_action::MessageSink;

/// The most items the lane holds while no in-game interface drains it: the oldest go first.
const HELD: usize = 256;

/// What an item signals.
#[derive(Clone, Debug)]
pub(super) enum Fire {
    /// `GUILD_ROSTER_UPDATE` (`0x4d1160`): bare, or with arg1 for the stale roster, whose signal
    /// clears the request deadline first (`0x4d1170`).
    Roster(RosterUpdate),
    /// `GUILD_MOTD` with the text (`0x5e728d`).
    Motd(String),
    /// `GUILD_INVITE_REQUEST` with the inviter and the guild (`0x48f45c`).
    Invite { inviter: String, guild: String },
    /// `/ginfo`'s two chat lines (`0x5e6fb0`).
    Info(GuildInfo),
}

/// One queued write of the guild state, stamped where its handler made it: the version it left,
/// and what it signals, if anything.
struct Item {
    at: Stamp,
    view: GuildView,
    fire: Option<Fire>,
}

/// The guild lane's queue, held in [`GuildState`].
pub(super) struct GuildLane {
    items: VecDeque<Item>,
    /// The version the last fired item left, the state the drain's start reads.
    base: GuildView,
    /// A drain is under way: the getters read [`Self::base`], not the live state.
    draining: AtomicBool,
}

impl GuildLane {
    /// A lane whose base is `view`.
    pub(super) fn new(view: GuildView) -> Self {
        Self {
            items: VecDeque::new(),
            base: view,
            draining: AtomicBool::new(false),
        }
    }

    /// The version the newest queued write left, else the base.
    fn tail(&self) -> &GuildView {
        self.items.back().map_or(&self.base, |i| &i.view)
    }

    /// Queue `view` stamped now, with what it signals.
    pub(super) fn push(&mut self, view: GuildView, fire: Option<Fire>) {
        if self.items.len() >= HELD {
            if let Some(dropped) = self.items.pop_front() {
                self.base = dropped.view;
            }
        }
        self.items.push_back(Item {
            at: crate::lanes::stamp(),
            view,
            fire,
        });
    }

    /// Whether `view` is the version the lane last saw.
    pub(super) fn is_tail(&self, view: &GuildView) -> bool {
        self.tail().same(view)
    }

    /// The oldest item's stamp.
    pub(super) fn head(&self) -> Option<Stamp> {
        self.items.front().map(|i| i.at)
    }

    /// The version the getters read: the last fired item's while a drain is under way.
    pub(super) fn drained(&self) -> Option<&GuildView> {
        self.draining.load(Ordering::Relaxed).then_some(&self.base)
    }

    /// Take the oldest item if `until` admits it, its version now the state the getters read.
    fn take(&mut self, until: Until) -> Option<(Stamp, Option<Fire>)> {
        if !until.admits(self.head()?) {
            return None;
        }
        let item = self.items.pop_front()?;
        self.base = item.view;
        Some((item.at, item.fire))
    }

    /// Begin a drain: the state before the first item.
    fn begin(&self) {
        self.draining.store(true, Ordering::Relaxed);
    }

    /// End a drain, however it ended.
    fn end(&self) {
        self.draining.store(false, Ordering::Relaxed);
    }
}

/// The lane, its drain's bounds and its firing.
pub(super) fn add_guild_lane(app: &mut App) {
    app.add_lane(
        "guild",
        |world| world.get_resource::<GuildState>()?.lane.head(),
        fire_guild,
        Some(|world| {
            if let Some(guild) = world.get_resource::<GuildState>() {
                guild.lane.begin();
            }
        }),
    )
    .add_drain_end(|world| {
        if let Some(guild) = world.get_resource::<GuildState>() {
            guild.lane.end();
        }
    });
}

/// The lane's firing: each item the drain admits, with its version the state the getters read,
/// its event signalled with the host lent. It stops after an item whose firing stamped anything,
/// so what that pushed fires next.
fn fire_guild(
    In(until): In<Until>,
    mut vm: VmWith<ResMut<'static, GuildState>>,
    mut sink: MessageSink,
) {
    if !vm.run(|script| script.is_some()) {
        return;
    }
    loop {
        let Some((at, fire)) = vm.own().lane.take(until) else {
            return;
        };
        let Some(fire) = fire else {
            continue;
        };
        if matches!(&fire, Fire::Roster(kind) if kind.arg1()) {
            vm.own().allow_roster_request();
        }
        let ((), moved) = crate::lanes::firing(at, || {
            vm.run(|script| {
                let Some(script) = script else {
                    return;
                };
                match fire {
                    Fire::Roster(kind) if kind.arg1() => {
                        script.fire_event("GUILD_ROSTER_UPDATE", vec![ScriptValue::Int(1)]);
                    }
                    Fire::Roster(_) => script.fire_event("GUILD_ROSTER_UPDATE", vec![]),
                    Fire::Motd(motd) => {
                        script.fire_event("GUILD_MOTD", vec![ScriptValue::Str(motd)]);
                    }
                    Fire::Invite { inviter, guild } => script.fire_event(
                        "GUILD_INVITE_REQUEST",
                        vec![ScriptValue::Str(inviter), ScriptValue::Str(guild)],
                    ),
                    Fire::Info(info) => {
                        let lines = ginfo_lines(script, &info);
                        crate::ui_action::show_messages(script, &mut sink, "ui_guild", lines);
                    }
                }
            });
        });
        if moved {
            return;
        }
    }
}

/// `SMSG_GUILD_INFO`'s chat lines, `GUILD_NAME_TEMPLATE` then `GUILD_INFO_TEMPLATE` (`0x5e6fb0`,
/// at `0x5e700f` and `0x5e706b`). Not message records: the handler reads each template from the
/// script VM (`0x703bf0`) and prints chat itself, so a missing template prints nothing. It passes
/// wire fields 2, 1, 3, 4, 5 (`0x5e704d`-`0x5e7061`): month first, where the wire has day first.
fn ginfo_lines(script: &UiScript, info: &GuildInfo) -> Vec<crate::ui_action::Shown> {
    use benilla_ui::strings::Arg;
    let get = |key: &str| script.lua().globals().get::<String>(key).ok();
    let line = |template: Option<String>, args: &[Arg<'_>]| {
        let text = benilla_ui::strings::fill(&template?, args);
        (!text.is_empty())
            .then(|| crate::ui_action::Shown::unkeyed(benilla_ui::messages::MsgKind::Chat, text))
    };
    let d = |v: u32| Arg::D(i64::from(v));
    [
        line(get("GUILD_NAME_TEMPLATE"), &[Arg::S(&info.name)]),
        line(
            get("GUILD_INFO_TEMPLATE"),
            &[
                d(info.created_month),
                d(info.created_day),
                d(info.created_year),
                d(info.member_count),
                d(info.account_count),
            ],
        ),
    ]
    .into_iter()
    .flatten()
    .collect()
}

#[cfg(test)]
impl Fire {
    /// A word for the fire, for the tests that list what a handler raised.
    pub(super) fn word(&self) -> String {
        match self {
            Self::Roster(RosterUpdate::Applied) => "roster".to_string(),
            Self::Roster(RosterUpdate::Stale) => "roster:stale".to_string(),
            Self::Motd(motd) => format!("motd:{motd}"),
            Self::Invite { inviter, guild } => format!("invite:{inviter}:{guild}"),
            Self::Info(info) => format!("info:{}", info.name),
        }
    }
}

#[cfg(test)]
impl GuildLane {
    /// What the queued items signal, in order, taking them.
    pub(super) fn take_words(&mut self) -> Vec<String> {
        let words = self
            .items
            .iter()
            .filter_map(|i| i.fire.as_ref().map(Fire::word))
            .collect();
        if let Some(last) = self.items.back() {
            self.base = last.view.clone();
        }
        self.items.clear();
        words
    }
}

/// The lane fired alone with everything stamped so far, for a test that drives the handlers
/// without the drain: the drain's begin and end around the firing.
#[cfg(test)]
pub(crate) fn fire_for_test(world: &mut World) {
    let id = world.register_system(fire_guild);
    if let Some(guild) = world.get_resource::<GuildState>() {
        guild.lane.begin();
    }
    world
        .run_system_with(id, Until::all())
        .expect("the guild lane fires");
    if let Some(guild) = world.get_resource::<GuildState>() {
        guild.lane.end();
    }
}
