//! The units' own watch lists: every watcher on a unit's fields is a record linked at the tail of
//! its list (`0x467e70` → `0x467f00`), and a change runs them in that order (`0x467f65`-
//! `0x467f76`). Three kinds watch a unit's health: its unit-field watchers, linked while its token
//! flags `+0xc90` are nonzero (`0x51bbb0`, from `0x60c4b0` and `0x60c65c`); each plate bar bound to
//! it (`0x7cc510`); and the unit tooltip's, linked by each build that shows it (`0x52a0a8`). Their
//! links share one order here, kept by the flags' writers as they write them.

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

/// The link order and the token flags the writers left.
#[derive(Clone, Debug, Default)]
pub(crate) struct UnitWatches {
    links: u64,
    flags: HashMap<u64, u32>,
    /// Each unit's unit-field watchers, by their link.
    bridges: HashMap<u64, u64>,
    /// Each tooltip's watcher on the unit it shows, by its link.
    tooltips: HashMap<FrameHandle, u64>,
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

    /// A tooltip build that shows a unit links the tooltip's watcher on it at the tail.
    pub(crate) fn link_tooltip(&mut self, tooltip: FrameHandle) {
        let at = self.link();
        self.tooltips.insert(tooltip, at);
    }

    /// The tooltip's clear unlinks its watcher (`0x53007c`).
    pub(crate) fn unlink_tooltip(&mut self, tooltip: FrameHandle) {
        self.tooltips.remove(&tooltip);
    }

    /// The tooltips whose watchers are linked, on whichever unit each shows.
    pub(crate) fn linked_tooltips(&self) -> impl Iterator<Item = FrameHandle> + '_ {
        self.tooltips.keys().copied()
    }

    /// The unit leaves the world: its object takes its list with it (`0x5fbb60`), the watchers
    /// of `tooltips`, which show it, included.
    pub(crate) fn gone(&mut self, guid: u64, tooltips: &[FrameHandle]) {
        self.flags.remove(&guid);
        self.bridges.remove(&guid);
        for h in tooltips {
            self.tooltips.remove(h);
        }
    }
}

/// The units' token flags and their unit-field watchers' links, as a VM leaves them for the next:
/// the reference keeps both on the unit objects (`+0xc90`, the lists `0x467f00` links onto), which
/// a `/reload` keeps. Its one token write is its deselect's (`0x490bdf` → `0x4939cc`); the group
/// writers run only in the group packets' handlers (`0x5e6a40`, `0x5e6850`, `0x5e6880`,
/// `0x5e68b0`). The plate bars' and the tooltips' links stay behind: the reload releases every
/// plate (`0x490c3a` → `0x60f890`) and each bind after it links its bar's watchers at the tail
/// again (`0x7cc3e2` → `0x7cc5d0`, `0x7cc416` → `0x7cc510`), and it destroys the tooltips, whose
/// destructor unlinks their watcher (`0x490c9b`, `0x529480` → `0x5294d5`).
#[derive(Clone, Debug, Default)]
pub struct TokenFlagsCarry {
    links: u64,
    flags: HashMap<u64, u32>,
    bridges: HashMap<u64, u64>,
}

/// What one health field's watchers write: the plate bars' range and value
/// (`0x7cc570`), absent for a unit no longer held; the tooltip's, on the health field alone
/// (`0x529560`, watching `+0x40`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HealthWatch {
    pub bar: Option<(f32, f32)>,
    pub tooltip: Option<(u32, u32)>,
}

/// One watcher on a unit's health, in its list.
enum Watcher {
    Bridge,
    Bar(FrameHandle),
    Tooltip(FrameHandle),
}

impl super::UiScript {
    /// The units' token flags and their unit-field watchers' links, taken from a VM that ends for a
    /// `/reload` ([`TokenFlagsCarry`]).
    pub fn take_token_flags(&mut self) -> TokenFlagsCarry {
        let mut model = self.model_mut();
        let watches = &mut model.unit_watches;
        TokenFlagsCarry {
            links: watches.links,
            flags: std::mem::take(&mut watches.flags),
            bridges: std::mem::take(&mut watches.bridges),
        }
    }

    /// Seat `carry` in a new VM before anything of it links: its own links follow the carried.
    pub fn seat_token_flags(&mut self, carry: TokenFlagsCarry) {
        let mut model = self.model_mut();
        let watches = &mut model.unit_watches;
        watches.links = watches.links.max(carry.links);
        watches.flags = carry.flags;
        watches.bridges = carry.bridges;
    }

    /// `guid`'s token flags as the writers left them, for a test.
    #[cfg(any(test, feature = "test-support"))]
    pub fn token_flags_of(&self, guid: u64) -> u32 {
        self.model_ref()
            .unit_watches
            .flags
            .get(&guid)
            .copied()
            .unwrap_or(0)
    }

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
            TokenChange::Add(bits) => old | bits,
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

    /// The watchers on one of `guid`'s health fields, in the order they were linked onto its list:
    /// `bridge`, its unit-field watchers' run (`0x51bd50`), at their link; each plate bar bound
    /// to it ([`super::nameplate`]); and on the health field each tooltip showing it, whose bar
    /// takes the range and value (`0x52958c`-`0x5295b1`). A unit with no watchers linked runs
    /// `bridge` first, which signals nothing for it.
    ///
    /// The walk reads each next watcher off the list as it stands after the one before returns
    /// (`0x4655dc` → `0x4655df`-`0x4655e6`): one an earlier watcher unlinked never runs, and one
    /// linked during the walk keeps a copy of the field as it now stands, so its compare finds no
    /// change (`0x4655bb`) and it runs neither.
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
        let (linked, last) = {
            let model = self.model_ref();
            let watches = &model.unit_watches;
            (watches.bridges.contains_key(&guid), watches.links)
        };
        let mut bridge = Some(bridge);
        if !linked {
            if let Some(bridge) = bridge.take() {
                bridge(self);
            }
        }
        let mut after = 0;
        while let Some((at, watcher)) = self.next_health_watcher(guid, watch, after, last) {
            after = at;
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
                Watcher::Tooltip(h) => {
                    if let Some((health, max)) = watch.tooltip {
                        super::tooltip_unit::watch_health(self.lua(), h, health, max);
                    }
                }
            }
        }
    }

    /// The first of `guid`'s health watchers still linked past link `after`, among those linked
    /// by link `last`, when the walk began.
    fn next_health_watcher(
        &self,
        guid: u64,
        watch: HealthWatch,
        after: u64,
        last: u64,
    ) -> Option<(u64, Watcher)> {
        let model = self.model_ref();
        let watches = &model.unit_watches;
        let bridge = watches.bridges.get(&guid).map(|&at| (at, Watcher::Bridge));
        let bars = watch
            .bar
            .map(|_| super::nameplate::bars_watching(&model, guid))
            .unwrap_or_default()
            .into_iter()
            .map(|(at, bar)| (at, Watcher::Bar(bar)));
        let tooltips = watches
            .tooltips
            .iter()
            .filter(|_| watch.tooltip.is_some())
            .filter(|&(&h, _)| super::tooltip_unit::shows(&model, h, guid))
            .map(|(&h, &at)| (at, Watcher::Tooltip(h)));
        bridge
            .into_iter()
            .chain(bars)
            .chain(tooltips)
            .filter(|&(at, _)| at > after && at <= last)
            .min_by_key(|&(at, _)| at)
    }
}
