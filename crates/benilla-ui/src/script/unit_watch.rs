//! The units' own watch lists: every watcher on a unit's fields is a record linked at the tail of
//! its list (`0x467e70` → `0x467f00`), and a change runs them in that order (`0x467f65`-
//! `0x467f76`). Two kinds watch a unit's health here: its unit-field watchers, linked while its
//! token flags `+0xc90` are nonzero (`0x51bbb0`, from `0x60c4b0` and `0x60c65c`), and each plate
//! bar bound to it (`0x7cc510`). Their links share one order, kept by the flags' writers as they
//! write them.

use std::collections::HashMap;

use crate::widget::FrameHandle;

/// The token flag bits (`0x60c520`): the active player, its pet, a party member and its pet, a
/// raid member and its pet, and the selection.
pub const TOKEN_PLAYER: u32 = 0x1;
pub const TOKEN_PET: u32 = 0x2;
pub const TOKEN_PARTY: u32 = 0x10;
pub const TOKEN_PARTY_PET: u32 = 0x20;
pub const TOKEN_RAID: u32 = 0x100;
pub const TOKEN_RAID_PET: u32 = 0x200;
pub const TOKEN_TARGET: u32 = 0x1000;

/// A write of a unit's token flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenChange {
    /// `AddUnitTokenFlags` (`0x60c4a0`): the selection's store (`0x493540`), a party seat
    /// (`0x4e839e`, its pet `0x4e83d6`), a raid arrival (`0x4bab2a`, its pet `0x4bab65`).
    Add(u32),
    /// `RemoveUnitTokenFlags` (`0x60c4f0`): the selection's teardown (`0x493910`), a party clear
    /// (`0x4e8561`, its pet `0x4e8530`), a raid departure (`0x4baa80`, its pet `0x4baa74`).
    Remove(u32),
    /// `UpdateUnitTokenFlags` (`0x60c520`), the flags read again from where they come from: a
    /// unit's create (`0x5fb880`), a charm or summon moving on the active player or a group
    /// member (`0x5e2702`, `0x5e2743`), a member record's pet (`0x5e5720`).
    Update,
}

/// Bit set on a unit whose tokens were found set with no writer's report, which links its watchers
/// there and keeps its flags nonzero until a writer's add accounts for them or a read of its
/// tokens finds them gone.
const UNREPORTED: u32 = 0x8000_0000;

/// The link order and the token flags the writers left.
#[derive(Clone, Debug, Default)]
pub(crate) struct UnitWatches {
    links: u64,
    flags: HashMap<u64, u32>,
    /// Each unit's unit-field watchers, by their link.
    bridges: HashMap<u64, u64>,
}

impl UnitWatches {
    /// The next link at a list's tail.
    pub(crate) fn link(&mut self) -> u64 {
        self.links += 1;
        self.links
    }

    /// The flags of `guid` become `new`: from 0 its watchers link (`0x60c4b0`, `0x60c65c`), to 0
    /// they go (`0x60c50c`, `0x60c679`).
    fn set_flags(&mut self, guid: u64, new: u32) {
        let old = self.flags.get(&guid).copied().unwrap_or(0);
        if old == new {
            return;
        }
        if old == 0 {
            let at = self.link();
            self.bridges.insert(guid, at);
        } else if new == 0 {
            self.bridges.remove(&guid);
        }
        if new == 0 {
            self.flags.remove(&guid);
        } else {
            self.flags.insert(guid, new);
        }
    }

    /// The unit leaves the world: its object takes its list with it (`0x5fbb60`).
    pub(crate) fn gone(&mut self, guid: u64) {
        self.flags.remove(&guid);
        self.bridges.remove(&guid);
    }
}

/// What one health field's watchers write: the plate bars' range and value (`0x7cc570`), absent
/// for a unit no longer held.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HealthWatch {
    pub bar: Option<(f32, f32)>,
}

/// One watcher on a unit's health, in its list.
enum Watcher {
    Bridge,
    Bar(FrameHandle),
}

impl super::UiScript {
    /// A writer's change of `guid`'s token flags, at its place.
    pub fn unit_token_flags(&mut self, guid: u64, change: TokenChange) {
        if guid == 0 {
            return;
        }
        let now = matches!(change, TokenChange::Update).then(|| self.token_flags_now(guid));
        let mut model = self.model_mut();
        let watches = &mut model.unit_watches;
        let old = watches.flags.get(&guid).copied().unwrap_or(0);
        let new = match change {
            TokenChange::Add(bits) => (old & !UNREPORTED) | bits,
            TokenChange::Remove(bits) => old & !bits,
            TokenChange::Update => now.unwrap_or(0),
        };
        watches.set_flags(guid, new);
    }

    /// The flags `0x60c520` reads of `guid` now, through the lent host, else off the snapshot.
    fn token_flags_now(&self, guid: u64) -> u32 {
        let model = self.model_ref();
        let guids = &model.unit_guids;
        let globals = super::unit::LiveGlobals::asked(self.lua(), guids);
        super::unit::with_live_objects(self.lua(), |o| {
            super::unit::token_flags_in(&globals, o, guid)
        })
        .unwrap_or_else(|| super::unit::token_flags_in(guids, guids, guid))
    }

    /// `guid`'s watchers as the tokens read now, for a unit no writer reported: linked at the
    /// tail when its tokens read set with none linked, gone when they read none.
    pub(crate) fn observe_bridge(&mut self, guid: u64) {
        let watched = self.unit_has_field_watchers(guid);
        let mut model = self.model_mut();
        let watches = &mut model.unit_watches;
        match (watched, watches.bridges.contains_key(&guid)) {
            (true, false) => watches.set_flags(guid, UNREPORTED),
            (false, true) => watches.set_flags(guid, 0),
            _ => {}
        }
    }

    /// The watchers on one of `guid`'s health fields, in the order they were linked onto its list:
    /// `bridge`, its unit-field watchers' run (`0x51bd50`), at their link; each plate bar bound
    /// to it ([`super::nameplate`]); and on the health field each tooltip showing it, whose bar
    /// takes the range and value (`0x52958c`-`0x5295b1`). A unit with no watchers linked runs
    /// `bridge` first, which signals nothing for it.
    pub fn unit_health_watchers(
        &mut self,
        guid: u64,
        watch: HealthWatch,
        bridge: impl FnOnce(&mut Self),
    ) {
        if guid == 0 {
            bridge(self);
            return;
        }
        self.observe_bridge(guid);
        let mut list: Vec<(u64, Watcher)> = {
            let model = self.model_ref();
            let watches = &model.unit_watches;
            let mut list = vec![(
                watches.bridges.get(&guid).copied().unwrap_or(0),
                Watcher::Bridge,
            )];
            if watch.bar.is_some() {
                list.extend(
                    super::nameplate::bars_watching(&model, guid)
                        .into_iter()
                        .map(|(at, bar)| (at, Watcher::Bar(bar))),
                );
            }
            list
        };
        list.sort_by_key(|&(at, _)| at);
        let mut bridge = Some(bridge);
        for (_, watcher) in list {
            match watcher {
                Watcher::Bridge => {
                    if let Some(bridge) = bridge.take() {
                        bridge(self);
                    }
                }
                Watcher::Bar(bar) => {
                    if let Some((health, max)) = watch.bar {
                        self.bar_watch(bar, health, max);
                    }
                }
            }
        }
    }
}
