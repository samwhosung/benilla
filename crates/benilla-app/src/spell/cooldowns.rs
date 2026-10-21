//! The player's cooldowns, the client's `SpellHistory` list (node ops `0x6e12c0`, `0x6e13e0`,
//! `0x6e1630`, `0x6e1790`; handlers `0x6e9460`, `0x6e95d0`, `0x6e9670`, `0x6e9730`).
//!
//! - A record holds three timers: the spell's recovery, its category's recovery and the GCD
//!   (`startRecoveryCategory`, `startRecoveryTime`); `on_hold` parks the first two until
//!   `SMSG_COOLDOWN_EVENT` (`SPELL_ATTR_COOLDOWN_ON_EVENT`: Stealth, Feign Death).
//! - The GCD starts at cast send (`0x6e58fb`); the spell's recovery is computed from `Spell.dbc`
//!   when our own `SMSG_SPELL_GO` arrives (`0x6e8498`, `0x6e8566`), an item's held until its
//!   record arrives (`0x6e8660`, `0x6e8830`). vmangos sends no cooldown packet for a plain cast;
//!   `SMSG_SPELL_COOLDOWN` is its override path.
//! - A failed cast clears the GCD armed at send (`0x6e1d83`, `0x6e1630`) and, unless the reason
//!   is 0x3c, removes a cooldown-on-event spell's parked record (`0x6e73cc`); a 0x3c whose first
//!   dword is 0 removes a held one (`0x6e1af1`).
//!
//! Each of the reference's sites of the flush `0x4b31b0` (`ACTIONBAR_UPDATE_COOLDOWN`,
//! `SPELL_UPDATE_COOLDOWN`) calls [`Cooldowns::flush`], whether or not its insert added a record:
//! the insert `0x6e12c0` and `StartCooldown 0x6e2c60` flush nothing, and nor does a natural expiry,
//! since the stock `Cooldown.lua` frame hides itself at the end.

use std::time::{Duration, Instant};

use bevy::prelude::*;

use super::mods::OP_COOLDOWN;
use super::{CategoryPad, SpellModifiers, OP_GCD};
use benilla_formats::SpellDisplay;
use benilla_protocol::messages::{ItemInfo, ItemSpellEntry, ItemUseSpell};

/// A signed millisecond time as a timer's duration, untracked at or below zero: a negative time
/// ends before its start, so no leg of a read passes it (`0x6e14f7`, `0x6e1588`, `0x6e15df`).
fn millis(ms: i32) -> Duration {
    Duration::from_millis(u64::from(ms.max(0).unsigned_abs()))
}

/// An item block's values as the go's item leg and its record callback take them
/// (`0x6e8511`-`0x6e8547`, `0x6e892a`-`0x6e896f`): its recovery and category recovery when not
/// negative and its category when positive, each else the spell's own column, unmodified.
fn item_leg(
    cooldown_ms: i32,
    category: u32,
    category_cooldown_ms: i32,
    spell: Option<&SpellDisplay>,
) -> (i32, u32, i32) {
    let recovery = if cooldown_ms >= 0 {
        cooldown_ms
    } else {
        spell.map_or(0, |s| s.recovery_ms as i32)
    };
    let category = if category as i32 > 0 {
        category
    } else {
        spell.map_or(0, |s| s.category)
    };
    let category_recovery = if category_cooldown_ms >= 0 {
        category_cooldown_ms
    } else {
        spell.map_or(0, |s| s.category_recovery_ms as i32)
    };
    (recovery, category, category_recovery)
}

/// One timer; a zero duration is untracked.
#[derive(Clone, Copy, Debug)]
struct Timer {
    start: Instant,
    duration: Duration,
}

impl Timer {
    fn none(now: Instant) -> Self {
        Self {
            start: now,
            duration: Duration::ZERO,
        }
    }

    fn remaining(&self, now: Instant) -> Duration {
        (self.start + self.duration).saturating_duration_since(now)
    }
}

/// One `SpellHistory` node (`0x6e12c0`).
#[derive(Clone, Debug)]
struct Record {
    spell_id: u32,
    /// The cast item's entry, 0 for a spell: records match on the spell and item pair.
    item_id: u32,
    recovery: Timer,
    category: u32,
    /// The category's `SpellCategory` row has flags bit 0x2, so it matches every query
    /// (`0x6e1563`); wand Shoot's 351 is the only one.
    category_wildcard: bool,
    category_recovery: Timer,
    /// The recovery timers hold their durations but have not started.
    on_hold: bool,
    gcd_category: u32,
    /// The GCD's time; it runs from [`Self::recovery`]'s start, the one `+0x10` start the
    /// reference's GCD leg reads (`0x6e15d8`), so `SMSG_COOLDOWN_EVENT` restarts it too.
    gcd: Duration,
}

impl Record {
    fn gcd_remaining(&self, now: Instant) -> Duration {
        (self.recovery.start + self.gcd).saturating_duration_since(now)
    }
}

/// One action's cooldown read (`GetActionCooldown`): the winning leg's start and duration, the
/// remainder to the latest end, and `enabled == false` once a parked leg passed, which
/// `CooldownFrame_SetTimer` hides. The start is what `GetCooldownInfo 0x6e13e0` returns, so two
/// arms never alias.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CooldownInfo {
    /// A parked leg reports the query's `now`.
    pub start: Instant,
    pub remaining_ms: u32,
    pub duration_ms: u32,
    pub enabled: bool,
}

impl CooldownInfo {
    /// `(start_ms on the GetTime clock, duration_ms, enabled)`, or `None` when cold. A held read
    /// (`enabled` false) pushes a start of 0: its start is the query's now (`0x6e1517`,
    /// `0x6e15a1`), which the Lua bindings answer at call time, so the pushed state never moves. `anchor` and
    /// `ui_now` must be the frame's one clock pair ([`crate::ui_script::UiClock`]), so one arm
    /// derives the same start every frame; a fresh `Instant::now()` here would jitter it.
    pub(crate) fn ui_triple(&self, anchor: Instant, ui_now: f64) -> Option<(i64, u32, bool)> {
        (self.remaining_ms > 0).then(|| {
            if !self.enabled {
                return (0, self.duration_ms, false);
            }
            // Signed both ways: a timer armed after the anchor sample projects forward.
            let start = match self.start.checked_duration_since(anchor) {
                Some(ahead) => ui_now + ahead.as_secs_f64(),
                None => ui_now - anchor.duration_since(self.start).as_secs_f64(),
            };
            #[allow(clippy::cast_possible_truncation)] // session-clock ms fit i64
            (
                (start * 1000.0).round() as i64,
                self.duration_ms,
                self.enabled,
            )
        })
    }
}

/// One cooldown list, the client's `SpellHistory`: the player's is this resource (`0xcecaec`), the
/// pet's is [`crate::ui_pet::PetBar::cooldowns`] (`0xcecb04`), which the pet bar, the pet book and
/// the pet's `SMSG_SPELL_GO` leg read and write. Each list's edges feed its own events: the
/// player's `feed_action_state`, the pet's `ui_pet::bar::fire_pet_cooldown_events`.
#[derive(Resource, Default)]
pub(crate) struct Cooldowns {
    records: Vec<Record>,
    /// Bumped by [`Self::flush`], the edge of `0x4b31b0`. Only counts up, across
    /// [`Self::clear_silent`] too, so a watcher reads a reset as no mutation.
    pub(crate) generation: u64,
    /// Bumped by the list's own calls of the usable refresh `0x4b31c0`, the edge of its
    /// `SPELL_UPDATE_USABLE`: a held global cooldown's start (`0x6e2e85`) and `0x6e3050`'s
    /// (`0x6e3076`). Only counts up.
    pub(crate) usable_refreshes: u64,
    /// Bumped by the list's calls of `0x4f93d0`, the edge of `BAG_UPDATE_COOLDOWN`:
    /// `SMSG_ITEM_COOLDOWN` (`0x6e965b`), the spell go's item leg (`0x6e8505`, `0x6e8570`),
    /// `0x6e3050` (`0x6e3089`) and the cheat (`0x6e9721`). The pet's list calls the pet's flush
    /// `0x4bce90` in its place (`0x6e3080`), so only the player's feed reads it. Only counts up.
    pub(crate) bag_flushes: u64,
    /// Bumped when [`Self::prune`] removes records. Kept apart from [`Self::generation`]: gated
    /// feeds must see an expiry, but the reference never fires the event for one.
    expiry_epoch: u64,
    /// Bumped by every record [`Self::add`] puts in. Kept apart from [`Self::generation`]: the
    /// insert flushes nothing (`0x6e12c0`), nor does `StartCooldown 0x6e2c60`'s seed (`0x6e2dcc`),
    /// but a gated feed must read the record.
    insert_epoch: u64,
    /// The item cooldowns the spell go's item leg holds until the item's record arrives, the
    /// table `0xceca60` of `ITEMCOOLDOWNHASHNODE`s keyed by item entry ([`Self::pend_item`]).
    pending_items: std::collections::HashMap<u32, PendingItem>,
    /// The item entries a cooldown query found without a record and asked with the callback
    /// `0x6e2f70` ([`Self::await_item_record`]). The item cache holds the callback, so it outlives
    /// the asking slot and a `/reload`.
    awaited_records: std::collections::HashSet<u32>,
}

/// `StartCooldown 0x6e2c60`'s own arguments: the spell and item, the category, the two remainders
/// and the held flag.
struct StartCooldown {
    spell_id: u32,
    item_id: u32,
    category: u32,
    recovery_ms: i32,
    category_recovery_ms: i32,
    held: bool,
}

/// One `ITEMCOOLDOWNHASHNODE` (`0x6e8660`): the spell at `+0x18`, the go's time at `+0x1c` and the
/// held flag at `+0x20`.
#[derive(Clone, Copy, Debug)]
struct PendingItem {
    spell_id: u32,
    at: Instant,
    on_hold: bool,
}

impl Cooldowns {
    /// `0x4b31b0`, the cooldown pair: `ACTIONBAR_UPDATE_COOLDOWN` and `SPELL_UPDATE_COOLDOWN`. On
    /// the pet's list its watcher adds `PET_BAR_UPDATE_COOLDOWN`, the `0x4bce90` each pet site
    /// calls after it.
    pub(crate) fn flush(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    fn refresh_usable(&mut self) {
        self.usable_refreshes = self.usable_refreshes.wrapping_add(1);
    }

    /// `0x4b31b0` then `0x4f93d0`, the pair every item leg calls.
    pub(crate) fn flush_item_leg(&mut self) {
        self.flush();
        self.bag_flushes = self.bag_flushes.wrapping_add(1);
    }

    /// `0x6e3050`'s tail after its walk, whether or not a record matched: the pair (`0x6e3071`),
    /// the usable refresh (`0x6e3076`), then the bags for the player (`0x6e3089`) or the pet's flush
    /// for the pet (`0x6e3080`), which reads [`Self::generation`].
    fn flush_records(&mut self) {
        self.refresh_usable();
        self.flush_item_leg();
    }

    /// How many records the list holds, elapsed ones included.
    #[cfg(test)]
    pub(crate) fn record_count(&self) -> usize {
        self.records.len()
    }

    /// The counter a gated feed watches: moves on any flush, insert or pruned expiry.
    pub(crate) fn feed_epoch(&self) -> u64 {
        self.generation
            .wrapping_add(self.expiry_epoch)
            .wrapping_add(self.insert_epoch)
    }

    /// An elapsed record awaits its prune. The feeds run before `feed_action_state`'s prune, so
    /// on the frame a timer crosses zero the read changes a frame before [`Self::feed_epoch`].
    pub(crate) fn sweep_pending(&self, now: Instant) -> bool {
        self.records.iter().any(|r| {
            !r.on_hold
                && r.recovery.remaining(now).is_zero()
                && r.category_recovery.remaining(now).is_zero()
                && r.gcd_remaining(now).is_zero()
        })
    }

    /// `AddCooldown 0x6e12c0`: always appends, never matching by id, so one spell holds separate
    /// nodes for its cast-send GCD and its SPELL_GO recovery. Replacing by id would let the GO
    /// insert wipe the running GCD.
    ///
    /// The list only grows here, so this is where it is bounded: the records that have run out by
    /// `now` are dropped first ([`Self::prune`]), and a list holds its live records plus those
    /// that ran out since its last insert. The pet's list has no other pruner.
    ///
    /// It flushes nothing: each caller's site does ([`Self::flush`]). It moves
    /// [`Self::feed_epoch`].
    fn add(
        &mut self,
        now: Instant,
        spell_id: u32,
        item_id: u32,
        recovery: Timer,
        category: u32,
        category_wildcard: bool,
        category_recovery: Timer,
        on_hold: bool,
        gcd_category: u32,
        gcd_ms: i32,
    ) {
        // The client's early-out (`0x6e12c3`-`0x6e12e0`): nothing goes in when the recovery and
        // category times, the held flag and the GCD time are all zero. A negative GCD time is not
        // zero (`0x6e12db`), so it goes in, with a GCD that ends before its start ([`millis`]).
        if recovery.duration.is_zero()
            && category_recovery.duration.is_zero()
            && !on_hold
            && gcd_ms == 0
        {
            return;
        }
        self.prune(now);
        self.records.push(Record {
            spell_id,
            item_id,
            recovery,
            category,
            category_wildcard,
            category_recovery,
            on_hold,
            gcd_category,
            gcd: millis(gcd_ms),
        });
        self.insert_epoch = self.insert_epoch.wrapping_add(1);
    }

    /// Drop fully elapsed records that are not on hold; invisible to reads, it stands in for the
    /// client's event-driven sweeps. Bumps [`Self::expiry_epoch`], not [`Self::generation`], so it
    /// flushes no event. [`Self::add`] runs it at every insert, and the player's feed each frame,
    /// which the gated feeds need to see an expiry with no insert.
    pub(crate) fn prune(&mut self, now: Instant) {
        let before = self.records.len();
        self.records.retain(|r| {
            r.on_hold
                || !r.recovery.remaining(now).is_zero()
                || !r.category_recovery.remaining(now).is_zero()
                || !r.gcd_remaining(now).is_zero()
        });
        if self.records.len() != before {
            self.expiry_epoch = self.expiry_epoch.wrapping_add(1);
        }
    }

    /// The go's spell leg (`0x6e8498`) or its pet leg (`0x6e85f7`): the spell's own cooldown from
    /// `Spell.dbc`, without a GCD, then the flush, inserted or not (`0x6e849d`, `0x6e85fc`).
    ///
    /// `ranged_attack_time_ms` is `UNIT_FIELD_RANGEDATTACKTIME` when
    /// [`SpellDisplay::ranged_speed_cooldown`], else 0, added to the category timer (`0x6e2b60`):
    /// the Throw and wand Shoot sweep. Category 0 (Auto Shot) never surfaces it. `mods` applies
    /// op 11 to both timers, as the self and pet legs do; `None` inserts the DBC values.
    pub(crate) fn start_spell(
        &mut self,
        spell_id: u32,
        spell: &SpellDisplay,
        ranged_attack_time_ms: u32,
        now: Instant,
        mods: Option<&SpellModifiers>,
    ) {
        let (recovery_ms, category_ms) = mods.map_or_else(
            || {
                (
                    spell.recovery_ms,
                    spell
                        .category_recovery_ms
                        .saturating_add(ranged_attack_time_ms),
                )
            },
            |mods| mods.spell_cooldowns(spell, ranged_attack_time_ms),
        );
        self.add(
            now,
            spell_id,
            0,
            Timer {
                start: now,
                duration: Duration::from_millis(u64::from(recovery_ms)),
            },
            spell.category,
            spell.category_wildcard,
            Timer {
                start: now,
                duration: Duration::from_millis(u64::from(category_ms)),
            },
            spell.cooldown_on_event(),
            0,
            0,
        );
        self.flush();
    }

    /// The go's item leg when the item's record names the spell (`0x6e8511`-`0x6e8566`), then
    /// its pair (`0x6e856b`, `0x6e8570`).
    pub(crate) fn start_item(
        &mut self,
        item_entry: u32,
        use_spell: &ItemUseSpell,
        spell: Option<&SpellDisplay>,
        now: Instant,
    ) {
        let (recovery_ms, category, category_ms) = item_leg(
            use_spell.cooldown_ms,
            use_spell.category,
            use_spell.category_cooldown_ms,
            spell,
        );
        self.add(
            now,
            use_spell.spell_id,
            item_entry,
            Timer {
                start: now,
                duration: millis(recovery_ms),
            },
            category,
            // Resolved through the spell only when the categories agree; no item carries 351.
            spell.is_some_and(|s| s.category == category && s.category_wildcard),
            Timer {
                start: now,
                duration: millis(category_ms),
            },
            spell.is_some_and(|s| s.cooldown_on_event()),
            0,
            0,
        );
        self.flush_item_leg();
    }

    /// Hold the go's item cooldown until the item's record arrives (`0x6e8660`): one node per item
    /// entry, found or added, which takes the spell, the go's time and the held flag.
    pub(crate) fn pend_item(
        &mut self,
        item_entry: u32,
        spell_id: u32,
        now: Instant,
        on_hold: bool,
    ) {
        self.pending_items.insert(
            item_entry,
            PendingItem {
                spell_id,
                at: now,
                on_hold,
            },
        );
    }

    /// The item cache's callback `0x6e8830` on a held item cooldown: the record of `item_entry`
    /// arrived, or was refused (`None`), and the node goes either way (`0x6e8a3a`).
    /// - A refusal finds no record and flushes nothing (`0x6e88d8`).
    /// - A record whose blocks name the held spell (`0x6e88f0`) inserts as the go's item leg does,
    ///   with the go's time and held flag, unless its recovery and category recovery are both 0
    ///   (`0x6e8971`-`0x6e897a`). The go asks with the argument 1 (`0x6e84b4`), which takes the
    ///   category pair (`0x6e8943`).
    /// - Any record then flushes the item leg's pair (`0x6e89a8`, `0x6e89ad`).
    pub(crate) fn item_record_landed<'a>(
        &mut self,
        item_entry: u32,
        record: Option<&ItemInfo>,
        spell: impl Fn(u32) -> Option<&'a SpellDisplay>,
    ) {
        let Some(held) = self.pending_items.remove(&item_entry) else {
            return;
        };
        let Some(record) = record else {
            return;
        };
        if let Some(block) = record.spells.iter().find(|b| b.spell_id == held.spell_id) {
            let spell = spell(held.spell_id);
            let (recovery_ms, category, category_ms) = item_leg(
                block.cooldown_ms,
                block.category,
                block.category_cooldown_ms,
                spell,
            );
            if recovery_ms != 0 || category_ms != 0 {
                self.add(
                    held.at,
                    held.spell_id,
                    item_entry,
                    Timer {
                        start: held.at,
                        duration: millis(recovery_ms),
                    },
                    category,
                    spell.is_some_and(|s| s.category == category && s.category_wildcard),
                    Timer {
                        start: held.at,
                        duration: millis(category_ms),
                    },
                    held.on_hold,
                    0,
                    0,
                );
            }
        }
        self.flush_item_leg();
    }

    /// A cooldown query (`0x6e2ed0`, `0x6e2fc0`) found the record of `item_entry` missing and asked
    /// it with the callback `0x6e2f70` (`0x6e2efb`, `0x6e2fea`).
    pub(crate) fn await_item_record(&mut self, item_entry: u32) {
        self.awaited_records.insert(item_entry);
    }

    /// The callback `0x6e2f70` of each awaited record the item cache now answers, spent either
    /// way: a record re-queries `0x6e2ed0` (`0x6e2f80`), which reads the first on-use spell with
    /// the item (`0x6e2f20`-`0x6e2f58`), and flushes the pair and the bags while a leg ends after
    /// now, running or held (`0x6e2f85`-`0x6e2f8e`); a refusal runs it with no record, which
    /// returns (`0x6e2f78`).
    pub(crate) fn answer_awaited_records(
        &mut self,
        items: &crate::items::Items,
        spells: Option<&benilla_formats::SpellCatalog>,
        now: Instant,
    ) {
        use crate::query_cache::Ask;
        let answered: Vec<u32> = self
            .awaited_records
            .iter()
            .copied()
            .filter(|&e| {
                items.template(e, 0, Ask::Read).is_some() || items.template_answered_unknown(e)
            })
            .collect();
        for entry in answered {
            self.awaited_records.remove(&entry);
            let Some(u) = items
                .template(entry, 0, Ask::Read)
                .and_then(|t| t.use_spell)
            else {
                continue;
            };
            let spell = spells.and_then(|s| s.get(u.spell_id));
            if self
                .info_item(u.spell_id, entry, u.category, spell, now)
                .remaining_ms
                > 0
            {
                self.flush_item_leg();
            }
        }
    }

    /// Arm the GCD at cast send (`StartGlobalCooldown 0x6e2de0`, from `0x6e58fb`) for any cast or
    /// item use, and at a pet-bar spell press (`0x4bd367`, into the pet's list), unless
    /// `startRecoveryCategory` and `startRecoveryTime` are both 0
    /// (`0x6e2e0f`-`0x6e2e21`), then with op 21 applied (`0x6e2e2c`) and nothing armed for a zero
    /// result (`0x6e2e3d`). A negative one is inserted, as the insert tests the GCD time for zero
    /// alone (`0x6e12db`), with a GCD no read shows (`0x6e15df`). Then the flush (`0x6e2e77`).
    /// `on_hold` is Attributes bit 25, which also runs the usable refresh after the flush
    /// (`0x6e2e7c`-`0x6e2e85`).
    pub(crate) fn start_gcd(
        &mut self,
        spell_id: u32,
        spell: &SpellDisplay,
        now: Instant,
        mods: &SpellModifiers,
    ) {
        if spell.start_recovery_category == 0 && spell.start_recovery_ms == 0 {
            return;
        }
        // `StartGlobalCooldown 0x6e2de0`: op 21 before the zero-duration exit (`0x6e2e2c`).
        let duration_ms = mods.apply(spell, OP_GCD, spell.start_recovery_ms as i32);
        if duration_ms == 0 {
            return;
        }
        if benilla_assets::trace::enabled() {
            benilla_assets::trace::line(
                "cd",
                &format!(
                    "arm-gcd spell={spell_id} gcdcat={} dur={}ms (cast-send)",
                    spell.start_recovery_category, duration_ms
                ),
            );
        }
        self.add(
            now,
            spell_id,
            0,
            Timer::none(now),
            0,
            false,
            Timer::none(now),
            spell.cooldown_on_event(),
            spell.start_recovery_category,
            duration_ms,
        );
        self.flush();
        if spell.cooldown_on_event() {
            self.refresh_usable();
        }
    }

    /// `ClearGlobalCooldown 0x6e1630`, on a failed cast: every record of the spell matches
    /// (`0x6e1658`), whatever its GCD, and has its GCD category and time zeroed (`0x6e1660`,
    /// `0x6e1663`); with a match, the sweep of the whole list at now, every spell's records
    /// (`0x6e1678` → `0x6e1910(list, now)`), and the flush (`0x6e167d`).
    pub(crate) fn clear_gcd(&mut self, spell_id: u32, now: Instant) {
        let mut touched = false;
        for r in &mut self.records {
            if r.spell_id == spell_id {
                r.gcd_category = 0;
                r.gcd = Duration::ZERO;
                touched = true;
            }
        }
        if touched {
            if benilla_assets::trace::enabled() {
                benilla_assets::trace::line(
                    "cd",
                    &format!("clear-gcd spell={spell_id} (cast-fail)"),
                );
            }
            self.prune(now);
            self.flush();
        }
    }

    /// `SMSG_COOLDOWN_EVENT` (`0x6e3050` → `0x6e1790`, force 0): an on-hold record's recovery and
    /// category starts become now (`0x6e185a`-`0x6e1863`), and with the recovery start the GCD
    /// leg's; then [`Self::flush_records`].
    pub(crate) fn cooldown_event(&mut self, spell_id: u32, now: Instant) {
        for r in &mut self.records {
            if r.spell_id == spell_id && r.on_hold {
                r.recovery.start = now;
                r.category_recovery.start = now;
                r.on_hold = false;
            }
        }
        self.flush_records();
    }

    /// `SMSG_CLEAR_COOLDOWN` and the cast-fail revert (`0x6e3050` → `0x6e1790`, force 1), then
    /// [`Self::flush_records`].
    pub(crate) fn clear_spell(&mut self, spell_id: u32) {
        self.records.retain(|r| r.spell_id != spell_id);
        self.flush_records();
    }

    /// `SMSG_COOLDOWN_CHEAT` (`0x6e9700`): the list emptied, then the pair (`0x6e9712`) and the
    /// bags for the player (`0x6e9721`) or the pet's flush, on an empty list too; no usable refresh.
    pub(crate) fn wipe(&mut self) {
        self.records.clear();
        self.flush_item_leg();
    }

    /// Empty the list with no flush: `SMSG_PET_SPELLS` clears the pet's list (`0x6e9b60` →
    /// `0x6e1880`) before it seeds it again, or for a teardown leaves it empty, and the packet
    /// calls no `0x4b31b0`. The counters keep their place, so a watcher never reads the reset as
    /// a mutation.
    pub(crate) fn clear_silent(&mut self) {
        self.records.clear();
    }

    /// Session end ([`crate::net::session::disconnected`]): `SMSG_INITIAL_SPELLS` re-sends every
    /// running cooldown at world entry and [`Self::seed_initial`] appends, so a surviving record
    /// would outlive the fresh one.
    pub(crate) fn clear_session(&mut self) {
        // The shutdown frees the held item cooldowns with both lists (`0x6e9ad0`).
        self.pending_items.clear();
        if !self.records.is_empty() {
            self.records.clear();
            self.flush();
        }
    }

    /// `SMSG_SPELL_COOLDOWN` (`0x6e9460`): each pair's record, a pair whose spell `Spell.dbc` lacks
    /// skipped (`0x6e94f8`), then the flush, whatever went in (`0x6e95b0`). `pad` is the list's
    /// [`CategoryPad`]: the pet's list passes `skip` (`0x6e9544`).
    pub(crate) fn apply_spell_cooldowns<'a>(
        &mut self,
        pairs: impl IntoIterator<Item = (u32, u32, Option<&'a SpellDisplay>)>,
        mods: &SpellModifiers,
        pad: CategoryPad,
        now: Instant,
    ) {
        for (spell_id, cooldown_ms, spell) in pairs {
            if let Some(spell) = spell {
                self.apply_wire_cooldown(spell_id, cooldown_ms, spell, mods, pad, now);
            }
        }
        self.flush();
    }

    /// One `SMSG_SPELL_COOLDOWN` entry, started at the packet:
    /// - recovery: a nonzero duration as it is, else `RecoveryTime` through op 11
    ///   (`0x6e950f`-`0x6e9523`);
    /// - category recovery: 0 for a nonzero duration, else `CategoryRecoveryTime`
    ///   (`0x6e952b`-`0x6e953b`), through `0x6e2b60` either way (`0x6e9544`), so the player's list
    ///   pads a ranged spell's whatever the duration;
    /// - held from Attributes bit 25 (`0x6e9504`), which takes no GCD (`0x6e9549`); any other the
    ///   pair `startRecoveryCategory`, `startRecoveryTime` through op 21 (`0x6e9553`-`0x6e956e`).
    fn apply_wire_cooldown(
        &mut self,
        spell_id: u32,
        cooldown_ms: u32,
        spell: &SpellDisplay,
        mods: &SpellModifiers,
        pad: CategoryPad,
        now: Instant,
    ) {
        let on_hold = spell.cooldown_on_event();
        let (recovery_ms, category_ms) = if cooldown_ms != 0 {
            (cooldown_ms as i32, mods.category_recovery(spell, 0, pad))
        } else {
            (
                mods.apply(spell, OP_COOLDOWN, spell.recovery_ms as i32),
                mods.category_recovery(spell, spell.category_recovery_ms as i32, pad),
            )
        };
        let (gcd_category, gcd_ms) = if on_hold {
            (0, 0)
        } else {
            (
                spell.start_recovery_category,
                mods.apply(spell, OP_GCD, spell.start_recovery_ms as i32),
            )
        };
        self.add(
            now,
            spell_id,
            0,
            Timer {
                start: now,
                duration: millis(recovery_ms),
            },
            spell.category,
            spell.category_wildcard,
            Timer {
                start: now,
                duration: millis(category_ms),
            },
            on_hold,
            gcd_category,
            gcd_ms,
        );
    }

    /// `SMSG_ITEM_COOLDOWN` (`0x6e95d0`): the client's hardcoded 30 s on the item's on-use spell,
    /// then the item leg's pair (`0x6e9656`, `0x6e965b`).
    pub(crate) fn apply_wire_item_cooldown(
        &mut self,
        item_entry: u32,
        spell_id: u32,
        now: Instant,
    ) {
        self.add(
            now,
            spell_id,
            item_entry,
            Timer {
                start: now,
                duration: Duration::from_millis(30_000),
            },
            0,
            false,
            Timer::none(now),
            false,
            0,
            0,
        );
        self.flush_item_leg();
    }

    /// One `SMSG_INITIAL_SPELLS` cooldown (`ReadInitialSpells 0x5e6510`): the category word's bit
    /// 31 is the held flag and the rest the remainder (`0x5e6632`-`0x5e664b`), seeded through
    /// [`Self::start_cooldown`] on the player's list. vmangos marks a cooldown-on-event spell
    /// running at login permanent: 1 ms and bit 31 (`Player.cpp:3499-3500`).
    pub(crate) fn seed_initial(
        &mut self,
        cd: &benilla_protocol::messages::SpellCooldown,
        spell: Option<&SpellDisplay>,
        block: Option<&ItemSpellEntry>,
        mods: &SpellModifiers,
        pad: CategoryPad,
        now: Instant,
    ) {
        let Some(spell) = spell else {
            return;
        };
        self.start_cooldown(
            StartCooldown {
                spell_id: u32::from(cd.spell_id),
                item_id: u32::from(cd.item_id),
                category: u32::from(cd.category),
                recovery_ms: cd.spell_cd_ms as i32,
                category_recovery_ms: (cd.category_cd_ms & 0x7fff_ffff) as i32,
                held: cd.category_cd_ms & 0x8000_0000 != 0,
            },
            spell,
            block,
            mods,
            pad,
            now,
        );
    }

    /// `StartCooldown 0x6e2c60`, cooldowns given as remainders, for a spell `Spell.dbc` holds
    /// (`0x6e2cbd`). `block` is the item's record's first slot naming the spell (`0x6e2cf4`).
    /// - A positive remainder gives a time of the larger of its base and it (`0x6e2d31`), the base
    ///   the slot's `+0x158` when not negative, else `RecoveryTime`, through op 11 (`0x6e2d40`),
    ///   started so the timer ends at the remainder: `now − time + remainder` (`0x6e2d4e`).
    /// - A positive category remainder the same, its base the slot's `+0x180` when not negative,
    ///   else `CategoryRecoveryTime` through `0x6e2b60` (`0x6e2d80`), then the larger
    ///   (`0x6e2d8b`), started likewise (`0x6e2d94`).
    /// - The record takes the caller's category and held flag and no GCD (`0x6e2dcc`). No flush.
    fn start_cooldown(
        &mut self,
        args: StartCooldown,
        spell: &SpellDisplay,
        block: Option<&ItemSpellEntry>,
        mods: &SpellModifiers,
        pad: CategoryPad,
        now: Instant,
    ) {
        // A timer of `time` that ends `remaining` from now.
        let ending = |time: i32, remaining: i32| -> Timer {
            let duration = millis(time);
            let end = now + millis(remaining);
            Timer {
                start: end.checked_sub(duration).unwrap_or(now),
                duration,
            }
        };
        let recovery = if args.recovery_ms > 0 {
            let base = block
                .map(|b| b.cooldown_ms)
                .filter(|&ms| ms >= 0)
                .unwrap_or(spell.recovery_ms as i32);
            let time = mods.apply(spell, OP_COOLDOWN, base.max(args.recovery_ms));
            ending(time, args.recovery_ms)
        } else {
            Timer::none(now)
        };
        let category_recovery = if args.category_recovery_ms > 0 {
            let base = block
                .map(|b| b.category_cooldown_ms)
                .filter(|&ms| ms >= 0)
                .unwrap_or_else(|| {
                    mods.category_recovery(spell, spell.category_recovery_ms as i32, pad)
                });
            ending(
                base.max(args.category_recovery_ms),
                args.category_recovery_ms,
            )
        } else {
            Timer::none(now)
        };
        self.add(
            now,
            args.spell_id,
            args.item_id,
            recovery,
            args.category,
            spell.category == args.category && spell.category_wildcard,
            category_recovery,
            args.held,
            0,
            0,
        );
    }

    /// One `SMSG_PET_SPELLS` cooldown: remainders, starting now. The category duration's
    /// [`PET_COOLDOWN_PERMANENT`] marker is stripped, or it would read as a 37-hour sweep.
    ///
    /// The handler seeds through `StartCooldown 0x6e2c60` (`0x4bdabf`), which calls no flush.
    pub(crate) fn seed_pet(
        &mut self,
        cd: &benilla_protocol::messages::PetSpellCooldown,
        spell: Option<&SpellDisplay>,
        now: Instant,
    ) {
        use benilla_protocol::messages::PET_COOLDOWN_PERMANENT;
        let category_ms = cd.category_cd_ms & !PET_COOLDOWN_PERMANENT;
        self.add(
            now,
            cd.spell_id,
            0,
            Timer {
                start: now,
                duration: Duration::from_millis(u64::from(cd.spell_cd_ms)),
            },
            u32::from(cd.category),
            // Off the catalog when present, else not a wildcard.
            spell.is_some_and(|s| s.category_wildcard),
            Timer {
                start: now,
                duration: Duration::from_millis(u64::from(category_ms)),
            },
            false,
            0,
            0,
        );
    }

    /// The client's `IsSpellOnCooldown 0x6e1690`, which despite its name is true only for an
    /// on-hold record; it feeds the usable check's grey-while-parked leg (`0x6e3fb1`).
    ///
    /// Keyed on spell and item (`0x6e173f`): the action bar's item gate `0x6e2fc0` passes the
    /// item entry (`0x6e3037`), every other caller 0. `category` is the item's for an item
    /// (`0x6e16f6` takes it from the item's spell slot), else the spell's.
    pub(crate) fn has_on_hold_record(&self, spell_id: u32, item_entry: u32, category: u32) -> bool {
        self.records.iter().any(|r| {
            r.on_hold
                && ((r.spell_id == spell_id && r.item_id == item_entry)
                    || (category != 0
                        && r.category == category
                        && !r.category_recovery.duration.is_zero()))
        })
    }

    /// The cast validator's first check (`0x6094f0` at `0x609565`, via `0x6e2ea0`): refused while
    /// [`Self::info`] reads any remainder, so a press sharing the running GCD's category is
    /// refused whatever its own `startRecoveryTime`. The caller picks the reason (`0x60952b`):
    /// 0x28 for an item, 0x3c for a spell. Refusing locally matters: the server's NOT_READY
    /// failure would clear the running GCD.
    /// `item_category` is [`Self::info_item`]'s: the item's own for an item press, else 0.
    pub(crate) fn not_ready(
        &self,
        spell_id: u32,
        item_entry: u32,
        item_category: u32,
        spell: Option<&SpellDisplay>,
        now: Instant,
    ) -> bool {
        self.info_item(spell_id, item_entry, item_category, spell, now)
            .remaining_ms
            > 0
    }

    /// `GetCooldownInfo 0x6e13e0`: the latest end over every record's three legs, in list order.
    ///
    /// - `Effect[0]` of ATTACK or TRADE_SKILL always reads cold (`0x6e1439`).
    /// - A leg passes when its end is not before the best so far (`0x6e14f7`, `0x6e1588`,
    ///   `0x6e15df`, `js`), so on a tie the later leg writes; it writes its duration and start.
    /// - Spell leg: id and item match, time or held nonzero (`0x6e14d2`). A parked record ends at
    ///   its time past `now + 1 ms` and writes a duration of 1 ms and a start of now
    ///   (`0x6e14ea`, `0x6e1503`, `0x6e1517`).
    /// - Category leg (`0x6e155a`-`0x6e156d`): equal nonzero category, or any query for a wildcard
    ///   row, with a category time. A parked record ends at its time past `now`, writes that time
    ///   and a start of now (`0x6e15a1`).
    /// - Both legs set `enabled` to "not parked" only while it is still true (`0x6e152d`,
    ///   `0x6e15b2`): one parked leg that passes disables the read for the rest of the walk.
    /// - GCD leg (`0x6e15cc`): the node's GCD category equals the query's `startRecoveryCategory`
    ///   and the node's time is nonzero; it ignores on-hold and never touches `enabled`. It runs
    ///   from the recovery start (`0x6e15d8`).
    /// - With an item, the category leg queries the item's own spell-slot category instead of the
    ///   spell's when that is positive (`0x6e1457`-`0x6e1499`): see [`Self::info_item`].
    pub(crate) fn info(
        &self,
        spell_id: u32,
        item_entry: u32,
        spell: Option<&SpellDisplay>,
        now: Instant,
    ) -> CooldownInfo {
        self.info_item(spell_id, item_entry, 0, spell, now)
    }

    /// [`Self::info`] for an item's on-use spell: a positive `item_category` (the item's
    /// `spellcategory` for that spell, `+0x16c` of its slot) replaces the spell's own category in
    /// the category leg (`0x6e1492`), as Fishliver Oil's 28 replaces the spell's 4. The GCD leg
    /// keeps the spell's `startRecoveryCategory`. 0 keeps the spell's.
    pub(crate) fn info_item(
        &self,
        spell_id: u32,
        item_entry: u32,
        item_category: u32,
        spell: Option<&SpellDisplay>,
        now: Instant,
    ) -> CooldownInfo {
        let mut out = CooldownInfo {
            start: now,
            remaining_ms: 0,
            duration_ms: 0,
            enabled: true,
        };
        if spell.is_some_and(|s| s.cooldown_query_excluded()) {
            return out;
        }
        let category = match item_category {
            0 => spell.map_or(0, |s| s.category),
            c => c,
        };
        let start_recovery_category = spell.map_or(0, |s| s.start_recovery_category);
        let ms = |d: Duration| d.as_millis().min(u128::from(u32::MAX)) as u32;
        let mut best_end = now;
        let mut leg = |end: Instant, start: Instant, duration: Duration| {
            if end < best_end {
                return false;
            }
            best_end = end;
            out.start = start;
            out.duration_ms = ms(duration);
            true
        };
        let mut enabled = true;
        for r in &self.records {
            if r.spell_id == spell_id
                && r.item_id == item_entry
                && (!r.recovery.duration.is_zero() || r.on_hold)
            {
                let passed = if r.on_hold {
                    let end = now + r.recovery.duration + Duration::from_millis(1);
                    leg(end, now, Duration::from_millis(1))
                } else {
                    let end = r.recovery.start + r.recovery.duration;
                    leg(end, r.recovery.start, r.recovery.duration)
                };
                if passed {
                    enabled &= !r.on_hold;
                }
            }
            if ((r.category != 0 && r.category == category) || r.category_wildcard)
                && !r.category_recovery.duration.is_zero()
            {
                let passed = if r.on_hold {
                    leg(
                        now + r.category_recovery.duration,
                        now,
                        r.category_recovery.duration,
                    )
                } else {
                    let end = r.category_recovery.start + r.category_recovery.duration;
                    leg(end, r.category_recovery.start, r.category_recovery.duration)
                };
                if passed {
                    enabled &= !r.on_hold;
                }
            }
            if r.gcd_category == start_recovery_category && !r.gcd.is_zero() {
                leg(r.recovery.start + r.gcd, r.recovery.start, r.gcd);
            }
        }
        out.remaining_ms = ms(best_end.saturating_duration_since(now));
        out.enabled = enabled;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spell(
        category: u32,
        recovery_ms: u32,
        category_recovery_ms: u32,
        gcd: (u32, u32),
        attributes: u32,
    ) -> SpellDisplay {
        SpellDisplay {
            category,
            recovery_ms,
            category_recovery_ms,
            start_recovery_category: gcd.0,
            start_recovery_ms: gcd.1,
            attributes,
            ..Default::default()
        }
    }

    /// Fireball-shaped: no own cooldown, the ordinary 133/1500 GCD.
    fn fireball() -> SpellDisplay {
        spell(0, 0, 0, (133, 1500), 0x10000)
    }

    #[test]
    fn gcd_uses_op_21_before_arming() {
        let mut d = fireball();
        d.spell_family = 3;
        d.spell_family_flags = 1 << 5;
        let mut mods = SpellModifiers::default();
        mods.set_class_family(3);
        mods.set(false, 5, OP_GCD, -50);
        let now = Instant::now();
        let mut cds = Cooldowns::default();
        cds.start_gcd(133, &d, now, &mods);
        assert_eq!(cds.info(133, 0, Some(&d), now).duration_ms, 750);

        d.start_recovery_ms = 0;
        mods.set(true, 5, OP_GCD, 1000);
        let mut cds = Cooldowns::default();
        cds.start_gcd(133, &d, now, &mods);
        assert_eq!(cds.info(133, 0, Some(&d), now).duration_ms, 500);
    }

    #[test]
    fn spell_cooldown_applies_op_11_to_both_timers_after_the_ranged_pad() {
        let mut d = spell(44, 8_000, 5_000, (0, 0), 0);
        d.spell_family = 3;
        d.spell_family_flags = 1 << 5;
        let mut mods = SpellModifiers::default();
        mods.set_class_family(3);
        mods.set(true, 5, super::super::OP_COOLDOWN, -1_500);
        assert_eq!(mods.spell_cooldowns(&d, 0), (6_500, 3_500));
        assert_eq!(mods.spell_cooldowns(&d, 2_000), (6_500, 5_500));

        let now = Instant::now();
        let mut cds = Cooldowns::default();
        cds.start_spell(2136, &d, 2_000, now, Some(&mods));
        assert_eq!(cds.info(2136, 0, Some(&d), now).duration_ms, 6_500);
        let sibling = spell(44, 0, 0, (0, 0), 0);
        assert_eq!(cds.info(999, 0, Some(&sibling), now).duration_ms, 5_500);
        let mut cds = Cooldowns::default();
        cds.start_spell(2136, &d, 2_000, now, None);
        assert_eq!(cds.info(2136, 0, Some(&d), now).duration_ms, 8_000);
        assert_eq!(cds.info(999, 0, Some(&sibling), now).duration_ms, 7_000);

        // Endurance 2/2: Sprint's five-minute recovery loses 30%.
        d.recovery_ms = 300_000;
        d.category_recovery_ms = 0;
        mods.set(true, 5, super::super::OP_COOLDOWN, 0);
        mods.set(false, 5, super::super::OP_COOLDOWN, -30);
        assert_eq!(mods.spell_cooldowns(&d, 0), (210_000, 0));
    }

    /// Charge-shaped: category 44, 15 s category cooldown, NO GCD pair.
    fn charge() -> SpellDisplay {
        spell(44, 0, 15_000, (0, 0), 0)
    }

    #[test]
    fn the_gcd_spreads_to_every_spell_sharing_the_start_recovery_category() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        cds.start_gcd(133, &fireball(), t0, &super::SpellModifiers::default());

        // Any spell with the same startRecoveryCategory reads the GCD; Charge has none.
        let mid = t0 + Duration::from_millis(500);
        let fb = cds.info(133, 0, Some(&fireball()), mid);
        assert_eq!(
            (fb.remaining_ms, fb.duration_ms, fb.enabled),
            (1000, 1500, true)
        );
        let frostbolt = fireball(); // same shape, different id
        let other = cds.info(116, 0, Some(&frostbolt), mid);
        assert_eq!((other.remaining_ms, other.duration_ms), (1000, 1500));
        let ch = cds.info(100, 0, Some(&charge()), mid);
        assert_eq!(ch.remaining_ms, 0, "no startRecoveryCategory — no GCD read");

        // The press gate reads the same getter; `0x6e1690` stays false with nothing parked.
        assert!(cds.not_ready(133, 0, 0, Some(&fireball()), mid));
        assert!(!cds.has_on_hold_record(133, 0, fireball().category));
    }

    /// The SPELL_GO insert (`StartCooldown`, no GCD) is its own node beside the cast-send GCD.
    #[test]
    fn a_go_self_insert_never_wipes_the_running_gcd() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        // Frost-Nova-shaped: own 25 s recovery, category 35, the ordinary 133/1500 GCD pair.
        let frost_nova = spell(35, 25_000, 0, (133, 1500), 0);
        cds.start_gcd(122, &frost_nova, t0, &super::SpellModifiers::default()); // the cast-send arm
        cds.start_spell(122, &frost_nova, 0, t0 + Duration::from_millis(100), None); // the GO insert

        let mid = t0 + Duration::from_millis(200);
        // Every GCD sibling still reads the running GCD.
        let fb = cds.info(133, 0, Some(&fireball()), mid);
        assert_eq!(
            (fb.remaining_ms, fb.duration_ms),
            (1300, 1500),
            "the GO self-insert must not eat the GCD node"
        );
        assert!(
            cds.not_ready(133, 0, 0, Some(&fireball()), mid),
            "the local lock holds too"
        );
        // Frost Nova's own button reads the longer, its own cooldown.
        let own = cds.info(122, 0, Some(&frost_nova), mid);
        assert_eq!((own.remaining_ms, own.duration_ms), (24_900, 25_000));
    }

    #[test]
    fn a_failed_cast_clears_the_gcd_and_the_optimistic_recovery() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        let fd = spell(0, 30_000, 0, (133, 1500), 0); // Feign-Death-shaped minus on-event
        cds.start_gcd(5384, &fd, t0, &super::SpellModifiers::default());
        cds.start_spell(5384, &fd, 0, t0, None);
        let mid = t0 + Duration::from_millis(100);
        assert!(cds.not_ready(5384, 0, 0, Some(&fd), mid));

        // The fail path (0x6e1a00): GCD cleared (0x6e1630), the record removed (0x6e3050).
        cds.clear_gcd(5384, mid);
        cds.clear_spell(5384);
        assert!(!cds.not_ready(5384, 0, 0, Some(&fd), mid));
        assert_eq!(cds.info(5384, 0, Some(&fd), mid).remaining_ms, 0);
    }

    #[test]
    fn category_cooldowns_reach_category_siblings_but_not_others() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        cds.start_spell(100, &charge(), 0, t0, None); // Charge: category 44, 15 s

        let mid = t0 + Duration::from_secs(5);
        // Another spell in category 44 reads the shared remainder.
        let sibling = spell(44, 0, 15_000, (0, 0), 0);
        let s = cds.info(999, 0, Some(&sibling), mid);
        assert_eq!((s.remaining_ms, s.duration_ms), (10_000, 15_000));
        assert!(
            cds.not_ready(999, 0, 0, Some(&sibling), mid),
            "category lock is a not-ready"
        );
        // An unrelated spell reads nothing.
        assert_eq!(cds.info(133, 0, Some(&fireball()), mid).remaining_ms, 0);
    }

    #[test]
    fn an_on_event_cooldown_parks_until_the_event_starts_it() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        // Feign Death: 30 s recovery, SPELL_ATTR_COOLDOWN_ON_EVENT (bit 25).
        let fd = spell(0, 30_000, 0, (0, 0), 0x0200_0000);
        cds.start_spell(5384, &fd, 0, t0, None);

        // Parked: the 30 s past now + 1 ms (`0x6e14ea`), disabled, and still not ready.
        let parked = cds.info(5384, 0, Some(&fd), t0 + Duration::from_secs(60));
        assert_eq!(
            (parked.remaining_ms, parked.enabled),
            (30_001, false),
            "an on-hold record never elapses on its own"
        );
        assert!(cds.not_ready(5384, 0, 0, Some(&fd), t0 + Duration::from_secs(60)));
        assert!(
            cds.has_on_hold_record(5384, 0, fd.category),
            "the corrected 0x6e1690: an on-hold record — the usable walk's grey-while-parked"
        );

        // SMSG_COOLDOWN_EVENT starts the clocks now.
        let event_at = t0 + Duration::from_secs(60);
        cds.cooldown_event(5384, event_at);
        let running = cds.info(5384, 0, Some(&fd), event_at + Duration::from_secs(10));
        assert_eq!(
            (running.remaining_ms, running.duration_ms, running.enabled),
            (20_000, 30_000, true)
        );
    }

    /// Stealth-shaped: 10 s recovery, GCD category 133 for 1 s, `SPELL_ATTR_COOLDOWN_ON_EVENT`.
    fn stealth() -> SpellDisplay {
        spell(0, 10_000, 0, (133, 1_000), 0x0200_0000)
    }

    /// `0x6e1503`, `0x6e1517`: a parked spell leg writes a duration of 1 ms and the query's now,
    /// not the record's recovery and insert time.
    #[test]
    fn a_parked_spell_leg_reads_one_ms_from_now() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        cds.start_spell(1784, &stealth(), 0, t0, None);

        let now = t0 + Duration::from_secs(7);
        let info = cds.info(1784, 0, Some(&stealth()), now);
        assert_eq!(
            (info.start, info.duration_ms, info.enabled),
            (now, 1, false),
            "(now, 0.001, 0), where the old read gave (insert time, 10, 0)"
        );
        assert_eq!(info.remaining_ms, 10_001, "the end is time + now + 1 ms");
        let (start, duration, enabled) = info.ui_triple(now, 42.0).unwrap();
        assert_eq!(
            (start, duration, enabled),
            (0, 1, false),
            "a held read pushes the marker, not a moving start"
        );
    }

    /// `0x6e15a1`: a parked category leg keeps the category time as the duration and reads a
    /// start of now.
    #[test]
    fn a_parked_category_leg_reads_its_time_from_now() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        // Recovery 0, category 12 for 5 s, parked.
        let parked = spell(12, 0, 5_000, (0, 0), 0x0200_0000);
        cds.start_spell(900, &parked, 0, t0, None);

        let now = t0 + Duration::from_secs(3);
        let sibling = spell(12, 0, 0, (0, 0), 0);
        let info = cds.info(901, 0, Some(&sibling), now);
        assert_eq!(
            (
                info.start,
                info.duration_ms,
                info.remaining_ms,
                info.enabled
            ),
            (now, 5_000, 5_000, false)
        );
    }

    /// `0x6e152d`, `0x6e15b2`: once a parked leg that passed has set enable to 0 a later leg that
    /// is not parked leaves it 0; in the other order the parked leg that ends earlier does not
    /// pass, so the read stays enabled.
    #[test]
    fn a_disabled_read_stays_disabled_whatever_leg_follows() {
        let t0 = Instant::now();
        let plain = |recovery| spell(0, recovery, 0, (0, 0), 0);
        let held = |recovery| spell(0, recovery, 0, (0, 0), 0x0200_0000);

        let mut cds = Cooldowns::default();
        cds.start_spell(77, &held(5_000), 0, t0, None);
        cds.start_spell(77, &plain(60_000), 0, t0, None);
        let info = cds.info(77, 0, Some(&plain(0)), t0);
        assert_eq!((info.remaining_ms, info.duration_ms), (60_000, 60_000));
        assert!(!info.enabled, "the later unparked leg does not re-enable");

        let mut cds = Cooldowns::default();
        cds.start_spell(77, &plain(60_000), 0, t0, None);
        cds.start_spell(77, &held(5_000), 0, t0, None);
        let info = cds.info(77, 0, Some(&plain(0)), t0);
        assert_eq!((info.remaining_ms, info.duration_ms), (60_000, 60_000));
        assert!(
            info.enabled,
            "5 s parked ends before the 60 s: it does not pass"
        );
    }

    /// A held read's start is the query's now, which moves with it; the triple a feed pushes is
    /// the held marker, so it does not move and the feeds see no change.
    #[test]
    fn a_held_triple_is_the_same_whatever_the_query_time() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        cds.start_spell(1784, &stealth(), 0, t0, None);
        let at = |ms: u64| {
            cds.info(1784, 0, Some(&stealth()), t0 + Duration::from_millis(ms))
                .ui_triple(t0, 7.0)
        };
        assert_ne!(
            cds.info(1784, 0, Some(&stealth()), t0).start,
            cds.info(1784, 0, Some(&stealth()), t0 + Duration::from_millis(20))
                .start,
            "the read's own start moves"
        );
        assert_eq!(at(0), at(20));
        assert_eq!(at(0), Some((0, 1, false)));
    }

    /// A parked record with no recovery (Shadowform) still passes the spell leg (`0x6e14d9`): the
    /// read is not cold and a press is refused.
    #[test]
    fn a_parked_record_with_no_recovery_still_reads() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        let form = spell(0, 0, 0, (133, 1_500), 0x0200_0000);
        cds.start_spell(15473, &form, 0, t0, None);

        let info = cds.info(15473, 0, Some(&form), t0);
        assert_eq!(
            (
                info.start,
                info.duration_ms,
                info.remaining_ms,
                info.enabled
            ),
            (t0, 1, 1, false)
        );
        assert!(cds.not_ready(15473, 0, 0, Some(&form), t0));
    }

    /// `0x6e185a`-`0x6e1863`, `0x6e15d8`: the event moves the one start the GCD leg reads, so the
    /// global cooldown runs again from the event.
    #[test]
    fn a_cooldown_event_restarts_the_gcd_from_the_event() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        let mods = SpellModifiers::default();
        let form = spell(0, 0, 0, (133, 1_500), 0x0200_0000);
        cds.start_gcd(15473, &form, t0, &mods);
        cds.start_spell(15473, &form, 0, t0, None);

        let event_at = t0 + Duration::from_secs(60);
        cds.cooldown_event(15473, event_at);

        let now = event_at + Duration::from_millis(500);
        let info = cds.info(15473, 0, Some(&form), now);
        assert_eq!(
            (
                info.start,
                info.duration_ms,
                info.remaining_ms,
                info.enabled
            ),
            (event_at, 1_500, 1_000, true)
        );
        // A sibling of the GCD category reads the same run.
        let sibling = spell(0, 0, 0, (133, 1_500), 0);
        assert_eq!(cds.info(133, 0, Some(&sibling), now).remaining_ms, 1_000);
        assert!(cds.not_ready(133, 0, 0, Some(&sibling), now));
    }

    #[test]
    fn wire_cooldowns_take_the_server_duration_or_fall_back_to_the_dbc() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        // A school lockout: nonzero wire ms is the recovery verbatim.
        let none = SpellModifiers::default();
        cds.apply_wire_cooldown(133, 8_000, &fireball(), &none, CategoryPad::NoPlayer, t0);
        let locked = cds.info(133, 0, Some(&fireball()), t0 + Duration::from_secs(3));
        assert_eq!((locked.remaining_ms, locked.duration_ms), (5_000, 8_000));

        // Zero wire ms: the spell's own Spell.dbc recovery/category pair.
        let mut cds = Cooldowns::default();
        cds.apply_wire_cooldown(100, 0, &charge(), &none, CategoryPad::NoPlayer, t0);
        let ch = cds.info(100, 0, Some(&charge()), t0 + Duration::from_secs(5));
        assert_eq!((ch.remaining_ms, ch.duration_ms), (10_000, 15_000));
    }

    #[test]
    fn a_session_end_empties_the_list_so_the_next_logins_wire_is_the_whole_truth() {
        use benilla_protocol::messages::SpellCooldown;
        let wire = |ms| SpellCooldown {
            spell_id: 12975, // Last Stand: 10 minutes, no category
            item_id: 0,
            category: 0,
            spell_cd_ms: ms,
            category_cd_ms: 0,
        };
        let last_stand = spell(0, 600_000, 0, (0, 0), 0);
        let seed = |cds: &mut Cooldowns, ms, at| {
            cds.seed_initial(
                &wire(ms),
                Some(&last_stand),
                None,
                &SpellModifiers::default(),
                CategoryPad::NoPlayer,
                at,
            );
        };
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();

        // Login one: 600 s still to run.
        seed(&mut cds, 600_000, t0);
        assert_eq!(
            cds.info(12975, 0, Some(&last_stand), t0).duration_ms,
            600_000
        );

        // Relog six seconds later: the server now says 594 s.
        let t1 = t0 + Duration::from_secs(6);
        cds.clear_session();
        assert!(cds.records.is_empty(), "the list dies with the session");
        seed(&mut cds, 594_000, t1);

        let read = cds.info(12975, 0, Some(&last_stand), t1);
        assert_eq!(
            (cds.records.len(), read.duration_ms, read.remaining_ms),
            (1, 600_000, 594_000),
            "one record, the second login's, its 10 minutes started to end at the remainder"
        );
    }

    /// `0x6e1690` matches the spell and item pair; only the item gate `0x6e2fc0` passes an entry.
    #[test]
    fn an_on_hold_record_is_findable_only_under_the_key_that_armed_it() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        // An on-event on-use spell, category 0 so the category leg cannot answer.
        let use_spell = ItemUseSpell {
            spell_id: 5384,
            cooldown_ms: 30_000,
            category: 0,
            category_cooldown_ms: 0,
        };
        let on_event = spell(0, 30_000, 0, (0, 0), 0x0200_0000);
        cds.start_item(1487, &use_spell, Some(&on_event), t0);

        assert!(
            cds.has_on_hold_record(5384, 1487, 0),
            "the ITEM gate's own query — `0x6e2fc0` passes the entry as itemId"
        );
        assert!(
            !cds.has_on_hold_record(5384, 0, 0),
            "the spell-slot form must not reach an item's record: itemId is part of the match"
        );
        assert!(
            !cds.has_on_hold_record(5384, 999, 0),
            "and not another item's entry either"
        );
    }

    #[test]
    fn item_use_cooldowns_key_on_the_item_and_respect_the_wire_triple() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        // A potion: category 4 for 60 s; the use cooldown -1 falls back to the spell's (none).
        let use_spell = ItemUseSpell {
            spell_id: 439,
            cooldown_ms: -1,
            category: 4,
            category_cooldown_ms: 60_000,
        };
        let potion_spell = spell(4, 0, 60_000, (133, 1500), 0);
        cds.start_item(118, &use_spell, Some(&potion_spell), t0);

        let mid = t0 + Duration::from_secs(15);
        // Spell 439 as cast from item 118 reads the category remainder.
        let info = cds.info(439, 118, Some(&potion_spell), mid);
        assert_eq!((info.remaining_ms, info.duration_ms), (45_000, 60_000));
        // So does any other category-4 potion.
        let other = cds.info(440, 929, Some(&potion_spell), mid);
        assert_eq!(other.remaining_ms, 45_000);
    }

    /// `0x6e1457`-`0x6e1499`: with an item, the category leg queries the item's own category for
    /// the spell (Fishliver Oil: spell category 4, item category 28), so the oil's record reaches
    /// a sibling of category 28 and no longer a bare category-4 spell.
    #[test]
    fn an_item_query_uses_the_items_category_override() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        let oil_spell = spell(4, 0, 0, (0, 0), 0);
        let oil = ItemUseSpell {
            spell_id: 1322,
            cooldown_ms: 0,
            category: 28,
            category_cooldown_ms: 20_000,
        };
        cds.start_item(1322, &oil, Some(&oil_spell), t0);
        let mid = t0 + Duration::from_secs(5);

        // The oil's own query, and a second item of category 28 over the same spell.
        let own = cds.info_item(1322, 1322, 28, Some(&oil_spell), mid);
        assert_eq!((own.remaining_ms, own.duration_ms), (15_000, 20_000));
        assert!(cds.not_ready(1322, 1322, 28, Some(&oil_spell), mid));
        // Another item whose slot carries category 28 reads the oil's category timer; the spell's
        // own category 4 does not match the record's 28.
        let other = cds.info_item(1322, 4096, 28, Some(&oil_spell), mid);
        assert_eq!(other.remaining_ms, 15_000);
        let by_spell = cds.info(1322, 4096, Some(&oil_spell), mid);
        assert_eq!(
            by_spell.remaining_ms, 0,
            "the spell's category 4 is not the record's 28"
        );
        // No override keeps the spell's category: a category-4 record reaches it.
        cds.start_spell(77, &spell(4, 0, 9_000, (0, 0), 0), 0, t0, None);
        assert_eq!(
            cds.info(1322, 4096, Some(&oil_spell), mid).remaining_ms,
            4_000
        );
    }

    #[test]
    fn prune_drops_only_fully_elapsed_records() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        cds.start_gcd(133, &fireball(), t0, &super::SpellModifiers::default());
        cds.start_spell(100, &charge(), 0, t0, None);
        assert_eq!(cds.records.len(), 2);

        // Past the 1.5 s GCD, inside Charge's 15 s.
        cds.prune(t0 + Duration::from_secs(5));
        assert_eq!(cds.records.len(), 1);
        assert_eq!(cds.records[0].spell_id, 100);

        cds.prune(t0 + Duration::from_secs(20));
        assert!(cds.records.is_empty());
    }

    /// The list is bounded where it grows: whatever the inserts' spacing, it holds the records
    /// live at the insert plus the insert itself, never every record ever added.
    #[test]
    fn an_insert_bounds_the_list_by_its_live_records_plus_itself() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        let mods = super::SpellModifiers::default();
        // Charge's 15 s category timer, then a 1.5 s GCD every 2 s for a minute.
        cds.start_spell(100, &charge(), 0, t0, None);
        for i in 0..30u64 {
            let now = t0 + Duration::from_secs(2 * i);
            cds.start_gcd(133, &fireball(), now, &mods);
            let charge_live = usize::from(2 * i < 15);
            assert_eq!(
                cds.record_count(),
                1 + charge_live,
                "insert {i}: Charge if it still runs, and the new GCD"
            );
        }
    }

    /// An insert drops the records that have run out and only those: a running timer and a parked
    /// record survive, no read of what stays changes, and the insert is the one mutation.
    #[test]
    fn an_insert_keeps_a_running_and_a_parked_record_and_changes_no_read() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        let mods = super::SpellModifiers::default();
        let brief = || spell(7, 2_000, 0, (0, 0), 0);
        // Parked: `Attributes` bit 25, its timers wait for `SMSG_COOLDOWN_EVENT`.
        let parked = || spell(9, 1_000, 0, (0, 0), 1 << 25);
        cds.start_gcd(133, &fireball(), t0, &mods);
        cds.start_spell(100, &brief(), 0, t0, None);
        cds.start_spell(200, &charge(), 0, t0, None);
        cds.start_spell(300, &parked(), 0, t0, None);
        assert_eq!(cds.record_count(), 4);

        let now = t0 + Duration::from_secs(5);
        let asks = [
            (133, fireball()),
            (100, brief()),
            (200, charge()),
            (201, spell(44, 0, 0, (0, 0), 0)),
            (300, parked()),
            (301, spell(9, 0, 0, (0, 0), 0)),
            (999, spell(0, 0, 0, (0, 0), 0)),
        ];
        let reads = |cds: &Cooldowns| {
            asks.iter()
                .map(|(id, d)| cds.info(*id, 0, Some(d), now))
                .collect::<Vec<_>>()
        };
        let before = reads(&cds);
        assert_eq!(before[2].remaining_ms, 10_000, "Charge runs");
        assert!(!before[4].enabled, "and the parked spell reads disabled");
        let generation = cds.generation;

        // A spell of no category the others share.
        cds.start_spell(500, &spell(99, 5_000, 0, (0, 0), 0), 0, now, None);

        assert_eq!(
            cds.record_count(),
            3,
            "Charge, the parked record and the new one"
        );
        assert!(!cds.sweep_pending(now), "nothing that ran out is left");
        assert_eq!(
            reads(&cds),
            before,
            "the records that stay read as they did"
        );
        assert_eq!(
            cds.generation,
            generation + 1,
            "one mutation, the insert's own"
        );
    }

    /// An elapsed record answers no read, so a prune cannot change one: what it keeps, a running
    /// timer and a parked record whose own timers have run down, answers as before.
    #[test]
    fn a_prune_changes_no_read() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        let mods = super::SpellModifiers::default();
        let brief = || spell(7, 2_000, 0, (0, 0), 0);
        // Parked: `Attributes` bit 25, its timers wait for `SMSG_COOLDOWN_EVENT`.
        let parked = || spell(9, 1_000, 0, (0, 0), 1 << 25);
        cds.start_gcd(133, &fireball(), t0, &mods);
        cds.start_spell(100, &brief(), 0, t0, None);
        cds.start_spell(200, &charge(), 0, t0, None);
        cds.start_spell(300, &parked(), 0, t0, None);
        assert_eq!(cds.records.len(), 4);

        let now = t0 + Duration::from_secs(5);
        let asks = [
            (133, fireball()),
            (100, brief()),
            (200, charge()),
            // Another spell of Charge's category, and one of the parked spell's.
            (201, spell(44, 0, 0, (0, 0), 0)),
            (300, parked()),
            (301, spell(9, 0, 0, (0, 0), 0)),
            (999, spell(0, 0, 0, (0, 0), 0)),
        ];
        let reads = |cds: &Cooldowns| {
            asks.iter()
                .map(|(id, d)| cds.info(*id, 0, Some(d), now))
                .collect::<Vec<_>>()
        };
        let before = reads(&cds);
        assert!(cds.sweep_pending(now), "the GCD and Brief have run out");
        assert_eq!(before[2].remaining_ms, 10_000, "Charge runs");
        assert!(!before[4].enabled, "and the parked spell reads disabled");

        cds.prune(now);

        assert_eq!(cds.records.len(), 2, "the GCD and Brief are gone");
        assert!(!cds.sweep_pending(now));
        assert_eq!(reads(&cds), before);
    }

    /// A prune that removes nothing must not move the epoch, or the per-frame prune would hold
    /// every gated feed open.
    #[test]
    fn the_feed_epoch_moves_on_expiry_but_an_empty_prune_is_silent() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        let e0 = cds.feed_epoch();
        cds.start_gcd(133, &fireball(), t0, &super::SpellModifiers::default());
        let armed = cds.feed_epoch();
        assert_ne!(e0, armed, "a mutation moves the epoch (generation)");

        cds.prune(t0 + Duration::from_millis(100));
        assert_eq!(
            cds.feed_epoch(),
            armed,
            "nothing elapsed — the prune is silent"
        );

        cds.prune(t0 + Duration::from_secs(5));
        let expired = cds.feed_epoch();
        assert_ne!(
            armed, expired,
            "the expiry IS a feed edge (the triple flips to None)"
        );
        assert_eq!(
            cds.generation,
            {
                let mut probe = Cooldowns::default();
                probe.start_gcd(133, &fireball(), t0, &super::SpellModifiers::default());
                probe.generation
            },
            "…but the event-edge generation never saw it (the reference is silent on expiry)"
        );

        cds.prune(t0 + Duration::from_secs(6));
        assert_eq!(cds.feed_epoch(), expired, "an empty prune stays silent");
    }

    /// A fail-clear and re-arm between two feeds must still read as a new start.
    #[test]
    fn the_ui_triple_is_stable_per_arm_and_distinct_across_arms() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        cds.start_gcd(772, &fireball(), t0, &super::SpellModifiers::default());

        // Two reads of one arm, frames apart, with both clocks in lockstep.
        let read1 = cds
            .info(772, 0, Some(&fireball()), t0 + Duration::from_millis(16))
            .ui_triple(t0 + Duration::from_millis(16), 10.016);
        let read2 = cds
            .info(772, 0, Some(&fireball()), t0 + Duration::from_millis(160))
            .ui_triple(t0 + Duration::from_millis(160), 10.160);
        assert_eq!(read1, Some((10_000, 1500, true)));
        assert_eq!(read1, read2, "one arm reads one start, every frame");

        // The fail clears the GCD and a re-press re-arms 200 ms later, unseen by the feed.
        cds.clear_gcd(772, t0 + Duration::from_millis(200));
        cds.start_gcd(
            772,
            &fireball(),
            t0 + Duration::from_millis(200),
            &super::SpellModifiers::default(),
        );
        let rearmed = cds
            .info(772, 0, Some(&fireball()), t0 + Duration::from_millis(216))
            .ui_triple(t0 + Duration::from_millis(216), 10.216);
        assert_eq!(
            rearmed,
            Some((10_200, 1500, true)),
            "a re-arm never aliases"
        );
    }

    #[test]
    fn a_mid_frame_arm_derives_the_same_start_as_the_next_frames_pair() {
        let anchor0 = Instant::now();
        let mut cds = Cooldowns::default();
        // Armed 4 ms after this frame's anchor sample.
        cds.start_gcd(
            133,
            &fireball(),
            anchor0 + Duration::from_millis(4),
            &super::SpellModifiers::default(),
        );

        // Frame 1 converts through the pre-arm pair, frame 2 through the next, 16 ms on.
        let read1 = cds
            .info(
                133,
                0,
                Some(&fireball()),
                anchor0 + Duration::from_millis(4),
            )
            .ui_triple(anchor0, 10.000);
        let read2 = cds
            .info(
                133,
                0,
                Some(&fireball()),
                anchor0 + Duration::from_millis(20),
            )
            .ui_triple(anchor0 + Duration::from_millis(16), 10.016);
        assert_eq!(read1, Some((10_004, 1500, true)));
        assert_eq!(read1, read2, "the projected start IS the settled start");
    }

    /// The GCD leg (`0x6e15cc`) ignores the pressed spell's own time: a `{133, 0}` scroll press
    /// is refused, a `{0, 0}` press is not.
    #[test]
    fn a_running_gcd_locks_presses_by_category_equality_alone() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        assert!(
            !cds.not_ready(133, 0, 0, Some(&fireball()), t0),
            "no GCD running — nothing locks"
        );

        cds.start_gcd(772, &fireball(), t0, &super::SpellModifiers::default());
        let mid = t0 + Duration::from_millis(200);
        assert!(
            cds.not_ready(133, 0, 0, Some(&fireball()), mid),
            "the spam press 200 ms later is locked — refused, never sent, the GCD lives"
        );
        // Scroll of Armor's shape.
        let scroll = spell(0, 0, 0, (133, 0), 0x10000);
        assert!(
            cds.not_ready(8091, 0, 0, Some(&scroll), mid),
            "a zero-GCD press in the shared category is locked on the reference"
        );
        assert!(!cds.not_ready(100, 0, 0, Some(&charge()), mid));
        assert!(!cds.not_ready(
            133,
            0,
            0,
            Some(&fireball()),
            t0 + Duration::from_millis(1_501)
        ));
    }

    /// The getter's exclusion (`0x6e1439`, `0x6e1442`).
    #[test]
    fn attack_and_tradeskill_reads_are_always_cold() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        cds.start_gcd(772, &fireball(), t0, &super::SpellModifiers::default());
        let mid = t0 + Duration::from_millis(200);
        let attack = SpellDisplay {
            effects: [78, 0, 0], // SPELL_EFFECT_ATTACK
            start_recovery_category: 133,
            ..Default::default()
        };
        assert_eq!(cds.info(6603, 0, Some(&attack), mid).remaining_ms, 0);
        assert!(!cds.not_ready(6603, 0, 0, Some(&attack), mid));
    }

    /// The wildcard leg (`0x6e1563`): wand Shoot's category 351 sweeps every button.
    #[test]
    fn a_wildcard_category_record_reaches_every_query() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        // Shoot: no DBC recovery; the ranged pad is the category duration.
        let shoot = SpellDisplay {
            category: 351,
            category_wildcard: true,
            attributes: 0x50012,
            ..Default::default()
        };
        cds.start_spell(5019, &shoot, 1500, t0, None);
        let mid = t0 + Duration::from_millis(500);
        // Unrelated Fireball reads the wand swing and is not ready for its duration.
        let fb = cds.info(133, 0, Some(&fireball()), mid);
        assert_eq!((fb.remaining_ms, fb.duration_ms), (1000, 1500));
        assert!(cds.not_ready(133, 0, 0, Some(&fireball()), mid));
        assert!(!cds.not_ready(
            133,
            0,
            0,
            Some(&fireball()),
            t0 + Duration::from_millis(1_501)
        ));
    }

    /// `0x6e3050`, behind `SMSG_COOLDOWN_EVENT`, `SMSG_CLEAR_COOLDOWN` and the cast-fail revert,
    /// flushes after its walk whether or not a record matched (`0x6e3071`), and so does the cheat
    /// (`0x6e9712`). The session end is no packet: it flushes only when something went.
    #[test]
    fn the_record_handlers_flush_whether_or_not_a_record_matched() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        let mut last = cds.generation;
        let mut moved = |cds: &Cooldowns| {
            let moved = cds.generation != last;
            last = cds.generation;
            moved
        };
        cds.clear_spell(133);
        assert!(moved(&cds), "a clear of nothing");
        cds.cooldown_event(133, t0);
        assert!(moved(&cds), "an event with nothing parked");
        cds.wipe();
        assert!(moved(&cds), "the cheat on an empty list");
        cds.clear_session();
        assert!(!moved(&cds), "a session end with nothing held");

        cds.start_gcd(133, &fireball(), t0, &super::SpellModifiers::default());
        assert!(moved(&cds));
        cds.clear_session();
        assert!(moved(&cds), "a session end that drops a record");
        assert_eq!(cds.info(133, 0, Some(&fireball()), t0).remaining_ms, 0);
    }

    /// The ranged pad (`0x6e2b60`): Throw (category 76, no DBC recovery) sweeps the weapon speed.
    #[test]
    fn throw_sweeps_the_ranged_attack_time_via_its_category() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        let throw = spell(76, 0, 0, (0, 0), 0x410012);
        cds.start_spell(2764, &throw, 2200, t0, None);
        let mid = t0 + Duration::from_millis(1000);
        let info = cds.info(2764, 0, Some(&throw), mid);
        assert_eq!(info.duration_ms, 2200, "the sweep is the weapon speed");
        assert_eq!(info.remaining_ms, 1200);
        assert!(info.enabled, "running, not parked");
        assert!(cds.not_ready(2764, 0, 0, Some(&throw), mid));
        assert!(
            !cds.not_ready(2764, 0, 0, Some(&throw), t0 + Duration::from_millis(2300)),
            "free again after the weapon speed elapses"
        );
    }

    /// Auto Shot's category 0 has no `SpellCategory` row, so its padded timer never surfaces.
    #[test]
    fn auto_shot_category_zero_never_surfaces_its_pad() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        let auto_shot = spell(0, 0, 0, (0, 0), 0x50012);
        cds.start_spell(75, &auto_shot, 3200, t0, None);
        let mid = t0 + Duration::from_millis(100);
        assert_eq!(
            cds.info(75, 0, Some(&auto_shot), mid).remaining_ms,
            0,
            "no sweep for a category-0 record"
        );
        assert!(
            !cds.not_ready(75, 0, 0, Some(&auto_shot), mid),
            "no local refusal either"
        );
    }
}

#[cfg(test)]
#[path = "cooldowns_tests.rs"]
mod site_tests;
