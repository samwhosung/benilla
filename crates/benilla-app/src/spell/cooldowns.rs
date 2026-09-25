//! The player's cooldowns, the client's `SpellHistory` list (node ops `0x6e12c0`, `0x6e13e0`,
//! `0x6e1630`, `0x6e1790`; handlers `0x6e9460`, `0x6e95d0`, `0x6e9670`, `0x6e9730`).
//!
//! - A record holds three timers: the spell's recovery, its category's recovery and the GCD
//!   (`startRecoveryCategory`, `startRecoveryTime`); `on_hold` parks the first two until
//!   `SMSG_COOLDOWN_EVENT` (`SPELL_ATTR_COOLDOWN_ON_EVENT`: Stealth, Feign Death).
//! - The GCD starts at cast send (`0x6e58fb`); the spell's recovery is computed from `Spell.dbc`
//!   when our own `SMSG_SPELL_GO` arrives (`0x6e8498`, `0x6e8566`). vmangos sends no cooldown
//!   packet for a plain cast; `SMSG_SPELL_COOLDOWN` is its override path.
//! - A failed cast clears the GCD armed at send (`0x6e1d83`, `0x6e1630`) and, unless the reason
//!   is 0x3c, removes a cooldown-on-event spell's parked record (`0x6e73cc`).
//!
//! Every mutation bumps [`Cooldowns::generation`], the `ACTIONBAR_UPDATE_COOLDOWN` edge; a
//! natural expiry does not, since the stock `Cooldown.lua` frame hides itself at the end.

use std::time::{Duration, Instant};

use bevy::prelude::*;

use benilla_formats::SpellDisplay;
use benilla_protocol::messages::ItemUseSpell;

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
    gcd: Timer,
}

/// One action's cooldown read (`GetActionCooldown`): the winning timer's start, duration and
/// remainder, and `enabled == false` for an on-hold record, which `CooldownFrame_SetTimer`
/// hides. The start is what `GetCooldownInfo 0x6e13e0` returns, so two arms never alias.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CooldownInfo {
    /// For an on-hold record, when it was inserted.
    pub start: Instant,
    pub remaining_ms: u32,
    pub duration_ms: u32,
    pub enabled: bool,
}

impl CooldownInfo {
    /// `(start_ms on the GetTime clock, duration_ms, enabled)`, or `None` when cold. `anchor` and
    /// `ui_now` must be the frame's one clock pair ([`crate::ui_script::UiClock`]), so one arm
    /// derives the same start every frame; a fresh `Instant::now()` here would jitter it.
    pub(crate) fn ui_triple(&self, anchor: Instant, ui_now: f64) -> Option<(i64, u32, bool)> {
        (self.remaining_ms > 0).then(|| {
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

/// The player's cooldown list (the client's `SpellHistory` at `0xcecaec`); the pet list has no
/// consumer.
#[derive(Resource, Default)]
pub(crate) struct Cooldowns {
    records: Vec<Record>,
    /// Bumped on every mutation: the `ACTIONBAR_UPDATE_COOLDOWN` edge.
    pub(crate) generation: u64,
    /// Bumped when [`Self::prune`] removes records. Kept apart from [`Self::generation`]: gated
    /// feeds must see an expiry, but the reference never fires the event for one.
    expiry_epoch: u64,
}

impl Cooldowns {
    fn bump(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    /// The counter a gated feed watches: moves on any mutation or pruned expiry.
    pub(crate) fn feed_epoch(&self) -> u64 {
        self.generation.wrapping_add(self.expiry_epoch)
    }

    /// An elapsed record awaits its prune. The feeds run before `feed_action_state`'s prune, so
    /// on the frame a timer crosses zero the read changes a frame before [`Self::feed_epoch`].
    pub(crate) fn sweep_pending(&self, now: Instant) -> bool {
        self.records.iter().any(|r| {
            !r.on_hold
                && r.recovery.remaining(now).is_zero()
                && r.category_recovery.remaining(now).is_zero()
                && r.gcd.remaining(now).is_zero()
        })
    }

    /// `AddCooldown 0x6e12c0`: always appends, never matching by id, so one spell holds separate
    /// nodes for its cast-send GCD and its SPELL_GO recovery. Replacing by id would let the GO
    /// insert wipe the running GCD.
    fn add(
        &mut self,
        spell_id: u32,
        item_id: u32,
        recovery: Timer,
        category: u32,
        category_wildcard: bool,
        category_recovery: Timer,
        on_hold: bool,
        gcd_category: u32,
        gcd: Timer,
    ) {
        // The client's early-out when nothing is tracked (`0x6e12c3`).
        if recovery.duration.is_zero()
            && category_recovery.duration.is_zero()
            && !on_hold
            && gcd.duration.is_zero()
        {
            return;
        }
        self.records.push(Record {
            spell_id,
            item_id,
            recovery,
            category,
            category_wildcard,
            category_recovery,
            on_hold,
            gcd_category,
            gcd,
        });
        self.bump();
    }

    /// Drop fully elapsed records that are not on hold; invisible to reads, it stands in for the
    /// client's event-driven sweeps. Bumps [`Self::expiry_epoch`], not [`Self::generation`].
    pub(crate) fn prune(&mut self, now: Instant) {
        let before = self.records.len();
        self.records.retain(|r| {
            r.on_hold
                || !r.recovery.remaining(now).is_zero()
                || !r.category_recovery.remaining(now).is_zero()
                || !r.gcd.remaining(now).is_zero()
        });
        if self.records.len() != before {
            self.expiry_epoch = self.expiry_epoch.wrapping_add(1);
        }
    }

    /// A spell's own cooldown from `Spell.dbc` (`StartCooldown 0x6e2c60`), without a GCD.
    ///
    /// `ranged_attack_time_ms` is `UNIT_FIELD_RANGEDATTACKTIME` when
    /// [`SpellDisplay::ranged_speed_cooldown`], else 0, added to the category timer (`0x6e2b60`):
    /// the Throw and wand Shoot sweep. Category 0 (Auto Shot) never surfaces it.
    pub(crate) fn start_spell(
        &mut self,
        spell_id: u32,
        spell: &SpellDisplay,
        ranged_attack_time_ms: u32,
        now: Instant,
    ) {
        self.add(
            spell_id,
            0,
            Timer {
                start: now,
                duration: Duration::from_millis(u64::from(spell.recovery_ms)),
            },
            spell.category,
            spell.category_wildcard,
            Timer {
                start: now,
                duration: Duration::from_millis(u64::from(
                    spell.category_recovery_ms + ranged_attack_time_ms,
                )),
            },
            spell.cooldown_on_event(),
            0,
            Timer::none(now),
        );
    }

    /// An item use's cooldown (`StartCooldown 0x6e2c60`): the item's values, a negative one
    /// falling back to the spell's `Spell.dbc` value.
    pub(crate) fn start_item(
        &mut self,
        item_entry: u32,
        use_spell: &ItemUseSpell,
        spell: Option<&SpellDisplay>,
        now: Instant,
    ) {
        let recovery_ms = if use_spell.cooldown_ms >= 0 {
            use_spell.cooldown_ms as u32
        } else {
            spell.map_or(0, |s| s.recovery_ms)
        };
        let category_ms = if use_spell.category_cooldown_ms >= 0 {
            use_spell.category_cooldown_ms as u32
        } else {
            spell.map_or(0, |s| s.category_recovery_ms)
        };
        self.add(
            use_spell.spell_id,
            item_entry,
            Timer {
                start: now,
                duration: Duration::from_millis(u64::from(recovery_ms)),
            },
            use_spell.category,
            // Resolved through the spell only when the categories agree; no item carries 351.
            spell.is_some_and(|s| s.category == use_spell.category && s.category_wildcard),
            Timer {
                start: now,
                duration: Duration::from_millis(u64::from(category_ms)),
            },
            spell.is_some_and(|s| s.cooldown_on_event()),
            0,
            Timer::none(now),
        );
    }

    /// Arm the GCD at cast send (`StartGlobalCooldown 0x6e2de0`, from `0x6e58fb`) for any cast,
    /// item use or pet cast, only when `startRecoveryTime != 0` (`0x6e2e0f`). `on_hold` is
    /// Attributes bit 25.
    pub(crate) fn start_gcd(&mut self, spell_id: u32, spell: &SpellDisplay, now: Instant) {
        if spell.start_recovery_ms == 0 {
            return;
        }
        if benilla_assets::trace::enabled() {
            benilla_assets::trace::line(
                "cd",
                &format!(
                    "arm-gcd spell={spell_id} gcdcat={} dur={}ms (cast-send)",
                    spell.start_recovery_category, spell.start_recovery_ms
                ),
            );
        }
        self.add(
            spell_id,
            0,
            Timer::none(now),
            0,
            false,
            Timer::none(now),
            spell.cooldown_on_event(),
            spell.start_recovery_category,
            Timer {
                start: now,
                duration: Duration::from_millis(u64::from(spell.start_recovery_ms)),
            },
        );
    }

    /// Clear the GCD of a spell's records (`0x6e1630`), on a failed cast.
    pub(crate) fn clear_gcd(&mut self, spell_id: u32, now: Instant) {
        let mut touched = false;
        for r in &mut self.records {
            if r.spell_id == spell_id && !r.gcd.duration.is_zero() {
                // `0x6e1630` zeroes both the category and the time.
                r.gcd_category = 0;
                r.gcd = Timer::none(now);
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
            self.bump();
        }
    }

    /// `SMSG_COOLDOWN_EVENT` (`0x6e1790`, force 0): an on-hold record's timers start now.
    pub(crate) fn cooldown_event(&mut self, spell_id: u32, now: Instant) {
        let mut touched = false;
        for r in &mut self.records {
            if r.spell_id == spell_id && r.on_hold {
                r.recovery.start = now;
                r.category_recovery.start = now;
                r.on_hold = false;
                touched = true;
            }
        }
        if touched {
            self.bump();
        }
    }

    /// `SMSG_CLEAR_COOLDOWN` and the cast-fail revert (`0x6e1790`, force 1).
    pub(crate) fn clear_spell(&mut self, spell_id: u32) {
        let before = self.records.len();
        self.records.retain(|r| r.spell_id != spell_id);
        if self.records.len() != before {
            self.bump();
        }
    }

    /// `SMSG_COOLDOWN_CHEAT` (`0x6e9700`).
    pub(crate) fn wipe(&mut self) {
        if !self.records.is_empty() {
            self.records.clear();
            self.bump();
        }
    }

    /// Session end ([`crate::net::session::disconnected`]): `SMSG_INITIAL_SPELLS` re-sends every
    /// running cooldown at world entry and [`Self::seed_initial`] appends, so a surviving record
    /// would outlive the fresh one.
    pub(crate) fn clear_session(&mut self) {
        self.wipe();
    }

    /// One `SMSG_SPELL_COOLDOWN` entry (`0x6e9460`): a nonzero duration is the recovery with no
    /// category timer; zero means the spell's own `Spell.dbc` pair. An on-event spell parks with
    /// no GCD; any other carries its GCD.
    pub(crate) fn apply_wire_cooldown(
        &mut self,
        spell_id: u32,
        cooldown_ms: u32,
        spell: Option<&SpellDisplay>,
        now: Instant,
    ) {
        let on_hold = spell.is_some_and(|s| s.cooldown_on_event());
        let (recovery_ms, category, category_ms) = if cooldown_ms != 0 {
            (cooldown_ms, spell.map_or(0, |s| s.category), 0)
        } else {
            match spell {
                Some(s) => (s.recovery_ms, s.category, s.category_recovery_ms),
                None => (0, 0, 0),
            }
        };
        let (gcd_category, gcd_ms) = if on_hold {
            (0, 0)
        } else {
            match spell {
                Some(s) => (s.start_recovery_category, s.start_recovery_ms),
                None => (0, 0),
            }
        };
        self.add(
            spell_id,
            0,
            Timer {
                start: now,
                duration: Duration::from_millis(u64::from(recovery_ms)),
            },
            category,
            spell.is_some_and(|s| s.category == category && s.category_wildcard),
            Timer {
                start: now,
                duration: Duration::from_millis(u64::from(category_ms)),
            },
            on_hold,
            gcd_category,
            Timer {
                start: now,
                duration: Duration::from_millis(u64::from(gcd_ms)),
            },
        );
    }

    /// `SMSG_ITEM_COOLDOWN` (`0x6e95d0`): the client's hardcoded 30 s on the item's on-use spell.
    pub(crate) fn apply_wire_item_cooldown(
        &mut self,
        item_entry: u32,
        spell_id: u32,
        now: Instant,
    ) {
        self.add(
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
            Timer::none(now),
        );
    }

    /// One `SMSG_INITIAL_SPELLS` cooldown: the wire carries the remainder, so the record starts
    /// now. A permanent cooldown (the category word's top bit) arrives as 1 ms and is kept.
    pub(crate) fn seed_initial(
        &mut self,
        cd: &benilla_protocol::messages::SpellCooldown,
        now: Instant,
    ) {
        self.add(
            u32::from(cd.spell_id),
            u32::from(cd.item_id),
            Timer {
                start: now,
                duration: Duration::from_millis(u64::from(cd.spell_cd_ms)),
            },
            u32::from(cd.category & 0x7FFF),
            // No catalog here; the wildcard category 351 never arrives in this list.
            false,
            Timer {
                start: now,
                duration: Duration::from_millis(u64::from(cd.category_cd_ms)),
            },
            false,
            0,
            Timer::none(now),
        );
    }

    /// One `SMSG_PET_SPELLS` cooldown: remainders, starting now. The category duration's
    /// [`PET_COOLDOWN_PERMANENT`] marker is stripped, or it would read as a 37-hour sweep.
    pub(crate) fn seed_pet(
        &mut self,
        cd: &benilla_protocol::messages::PetSpellCooldown,
        spell: Option<&SpellDisplay>,
        now: Instant,
    ) {
        use benilla_protocol::messages::PET_COOLDOWN_PERMANENT;
        let category_ms = cd.category_cd_ms & !PET_COOLDOWN_PERMANENT;
        self.add(
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
            Timer::none(now),
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
    pub(crate) fn not_ready(
        &self,
        spell_id: u32,
        item_entry: u32,
        spell: Option<&SpellDisplay>,
        now: Instant,
    ) -> bool {
        self.info(spell_id, item_entry, spell, now).remaining_ms > 0
    }

    /// `GetCooldownInfo 0x6e13e0`: the longest remainder over every record's three legs.
    ///
    /// - `Effect[0]` of ATTACK or TRADE_SKILL always reads cold (`0x6e1439`).
    /// - Spell leg: id and item match; a parked record reads its full duration, disabled.
    /// - Category leg: equal category, or any query for a wildcard row; parked likewise.
    /// - GCD leg (`0x6e15cc`): the node's GCD category equals the query's
    ///   `startRecoveryCategory` and the node's time is nonzero; it ignores on-hold.
    pub(crate) fn info(
        &self,
        spell_id: u32,
        item_entry: u32,
        spell: Option<&SpellDisplay>,
        now: Instant,
    ) -> CooldownInfo {
        let mut best = CooldownInfo {
            start: now,
            remaining_ms: 0,
            duration_ms: 0,
            enabled: true,
        };
        if spell.is_some_and(|s| s.cooldown_query_excluded()) {
            return best;
        }
        let category = spell.map_or(0, |s| s.category);
        let start_recovery_category = spell.map_or(0, |s| s.start_recovery_category);
        let mut consider = |timer: &Timer, remaining: Duration, enabled: bool| {
            let remaining_ms = remaining.as_millis().min(u128::from(u32::MAX)) as u32;
            if remaining_ms > best.remaining_ms {
                best = CooldownInfo {
                    start: timer.start,
                    remaining_ms,
                    duration_ms: timer.duration.as_millis().min(u128::from(u32::MAX)) as u32,
                    enabled,
                };
            }
        };
        for r in &self.records {
            if r.spell_id == spell_id && r.item_id == item_entry {
                if r.on_hold {
                    // Parked: full duration, disabled (enable 0 hides the sweep).
                    consider(&r.recovery, r.recovery.duration, false);
                } else {
                    consider(&r.recovery, r.recovery.remaining(now), true);
                }
            }
            if (r.category != 0 && r.category == category) || r.category_wildcard {
                if r.on_hold {
                    consider(&r.category_recovery, r.category_recovery.duration, false);
                } else {
                    consider(
                        &r.category_recovery,
                        r.category_recovery.remaining(now),
                        true,
                    );
                }
            }
            if r.gcd_category == start_recovery_category && !r.gcd.duration.is_zero() {
                consider(&r.gcd, r.gcd.remaining(now), true);
            }
        }
        best
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

    /// Charge-shaped: category 44, 15 s category cooldown, NO GCD pair.
    fn charge() -> SpellDisplay {
        spell(44, 0, 15_000, (0, 0), 0)
    }

    #[test]
    fn the_gcd_spreads_to_every_spell_sharing_the_start_recovery_category() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        cds.start_gcd(133, &fireball(), t0);

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
        assert!(cds.not_ready(133, 0, Some(&fireball()), mid));
        assert!(!cds.has_on_hold_record(133, 0, fireball().category));
    }

    /// The SPELL_GO insert (`StartCooldown`, no GCD) is its own node beside the cast-send GCD.
    #[test]
    fn a_go_self_insert_never_wipes_the_running_gcd() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        // Frost-Nova-shaped: own 25 s recovery, category 35, the ordinary 133/1500 GCD pair.
        let frost_nova = spell(35, 25_000, 0, (133, 1500), 0);
        cds.start_gcd(122, &frost_nova, t0); // the cast-send arm
        cds.start_spell(122, &frost_nova, 0, t0 + Duration::from_millis(100)); // the GO insert

        let mid = t0 + Duration::from_millis(200);
        // Every GCD sibling still reads the running GCD.
        let fb = cds.info(133, 0, Some(&fireball()), mid);
        assert_eq!(
            (fb.remaining_ms, fb.duration_ms),
            (1300, 1500),
            "the GO self-insert must not eat the GCD node"
        );
        assert!(
            cds.not_ready(133, 0, Some(&fireball()), mid),
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
        cds.start_gcd(5384, &fd, t0);
        cds.start_spell(5384, &fd, 0, t0);
        let mid = t0 + Duration::from_millis(100);
        assert!(cds.not_ready(5384, 0, Some(&fd), mid));

        // The fail path (0x6e1a00): GCD cleared (0x6e1630), the record removed (0x6e3050).
        cds.clear_gcd(5384, mid);
        cds.clear_spell(5384);
        assert!(!cds.not_ready(5384, 0, Some(&fd), mid));
        assert_eq!(cds.info(5384, 0, Some(&fd), mid).remaining_ms, 0);
    }

    #[test]
    fn category_cooldowns_reach_category_siblings_but_not_others() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        cds.start_spell(100, &charge(), 0, t0); // Charge: category 44, 15 s

        let mid = t0 + Duration::from_secs(5);
        // Another spell in category 44 reads the shared remainder.
        let sibling = spell(44, 0, 15_000, (0, 0), 0);
        let s = cds.info(999, 0, Some(&sibling), mid);
        assert_eq!((s.remaining_ms, s.duration_ms), (10_000, 15_000));
        assert!(
            cds.not_ready(999, 0, Some(&sibling), mid),
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
        cds.start_spell(5384, &fd, 0, t0);

        // Parked: full duration, disabled, and still not ready.
        let parked = cds.info(5384, 0, Some(&fd), t0 + Duration::from_secs(60));
        assert_eq!(
            (parked.remaining_ms, parked.enabled),
            (30_000, false),
            "an on-hold record never elapses on its own"
        );
        assert!(cds.not_ready(5384, 0, Some(&fd), t0 + Duration::from_secs(60)));
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

    #[test]
    fn wire_cooldowns_take_the_server_duration_or_fall_back_to_the_dbc() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        // A school lockout: nonzero wire ms is the recovery verbatim.
        cds.apply_wire_cooldown(133, 8_000, Some(&fireball()), t0);
        let locked = cds.info(133, 0, Some(&fireball()), t0 + Duration::from_secs(3));
        assert_eq!((locked.remaining_ms, locked.duration_ms), (5_000, 8_000));

        // Zero wire ms: the spell's own Spell.dbc recovery/category pair.
        let mut cds = Cooldowns::default();
        cds.apply_wire_cooldown(100, 0, Some(&charge()), t0);
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
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();

        // Login one: 600 s still to run.
        cds.seed_initial(&wire(600_000), t0);
        assert_eq!(cds.info(12975, 0, None, t0).duration_ms, 600_000);

        // Relog six seconds later: the server now says 594 s.
        let t1 = t0 + Duration::from_secs(6);
        cds.clear_session();
        assert!(cds.records.is_empty(), "the list dies with the session");
        cds.seed_initial(&wire(594_000), t1);

        let read = cds.info(12975, 0, None, t1);
        assert_eq!(
            (cds.records.len(), read.duration_ms, read.remaining_ms),
            (1, 594_000, 594_000),
            "one record, the second login's — a surviving first-login record would answer 600 s \
             and go on answering it for the whole cooldown"
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

    #[test]
    fn prune_drops_only_fully_elapsed_records() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        cds.start_gcd(133, &fireball(), t0);
        cds.start_spell(100, &charge(), 0, t0);
        assert_eq!(cds.records.len(), 2);

        // Past the 1.5 s GCD, inside Charge's 15 s.
        cds.prune(t0 + Duration::from_secs(5));
        assert_eq!(cds.records.len(), 1);
        assert_eq!(cds.records[0].spell_id, 100);

        cds.prune(t0 + Duration::from_secs(20));
        assert!(cds.records.is_empty());
    }

    /// A prune that removes nothing must not move the epoch, or the per-frame prune would hold
    /// every gated feed open.
    #[test]
    fn the_feed_epoch_moves_on_expiry_but_an_empty_prune_is_silent() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        let e0 = cds.feed_epoch();
        cds.start_gcd(133, &fireball(), t0);
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
                probe.start_gcd(133, &fireball(), t0);
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
        cds.start_gcd(772, &fireball(), t0);

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
        cds.start_gcd(772, &fireball(), t0 + Duration::from_millis(200));
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
        cds.start_gcd(133, &fireball(), anchor0 + Duration::from_millis(4));

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
            !cds.not_ready(133, 0, Some(&fireball()), t0),
            "no GCD running — nothing locks"
        );

        cds.start_gcd(772, &fireball(), t0);
        let mid = t0 + Duration::from_millis(200);
        assert!(
            cds.not_ready(133, 0, Some(&fireball()), mid),
            "the spam press 200 ms later is locked — refused, never sent, the GCD lives"
        );
        // Scroll of Armor's shape.
        let scroll = spell(0, 0, 0, (133, 0), 0x10000);
        assert!(
            cds.not_ready(8091, 0, Some(&scroll), mid),
            "a zero-GCD press in the shared category is locked on the reference"
        );
        assert!(!cds.not_ready(100, 0, Some(&charge()), mid));
        assert!(!cds.not_ready(133, 0, Some(&fireball()), t0 + Duration::from_millis(1_501)));
    }

    /// The getter's exclusion (`0x6e1439`, `0x6e1442`).
    #[test]
    fn attack_and_tradeskill_reads_are_always_cold() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        cds.start_gcd(772, &fireball(), t0);
        let mid = t0 + Duration::from_millis(200);
        let attack = SpellDisplay {
            effects: [78, 0, 0], // SPELL_EFFECT_ATTACK
            start_recovery_category: 133,
            ..Default::default()
        };
        assert_eq!(cds.info(6603, 0, Some(&attack), mid).remaining_ms, 0);
        assert!(!cds.not_ready(6603, 0, Some(&attack), mid));
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
        cds.start_spell(5019, &shoot, 1500, t0);
        let mid = t0 + Duration::from_millis(500);
        // Unrelated Fireball reads the wand swing and is not ready for its duration.
        let fb = cds.info(133, 0, Some(&fireball()), mid);
        assert_eq!((fb.remaining_ms, fb.duration_ms), (1000, 1500));
        assert!(cds.not_ready(133, 0, Some(&fireball()), mid));
        assert!(!cds.not_ready(133, 0, Some(&fireball()), t0 + Duration::from_millis(1_501)));
    }

    #[test]
    fn cheat_wipe_and_clear_bump_the_generation_only_on_change() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        let g0 = cds.generation;
        cds.clear_spell(133);
        cds.wipe();
        assert_eq!(cds.generation, g0);

        cds.start_gcd(133, &fireball(), t0);
        assert_ne!(cds.generation, g0);
        let g1 = cds.generation;
        cds.wipe();
        assert_ne!(cds.generation, g1);
        assert_eq!(cds.info(133, 0, Some(&fireball()), t0).remaining_ms, 0);
    }

    /// The ranged pad (`0x6e2b60`): Throw (category 76, no DBC recovery) sweeps the weapon speed.
    #[test]
    fn throw_sweeps_the_ranged_attack_time_via_its_category() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        let throw = spell(76, 0, 0, (0, 0), 0x410012);
        cds.start_spell(2764, &throw, 2200, t0);
        let mid = t0 + Duration::from_millis(1000);
        let info = cds.info(2764, 0, Some(&throw), mid);
        assert_eq!(info.duration_ms, 2200, "the sweep is the weapon speed");
        assert_eq!(info.remaining_ms, 1200);
        assert!(info.enabled, "running, not parked");
        assert!(cds.not_ready(2764, 0, Some(&throw), mid));
        assert!(
            !cds.not_ready(2764, 0, Some(&throw), t0 + Duration::from_millis(2300)),
            "free again after the weapon speed elapses"
        );
    }

    /// Auto Shot's category 0 has no `SpellCategory` row, so its padded timer never surfaces.
    #[test]
    fn auto_shot_category_zero_never_surfaces_its_pad() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        let auto_shot = spell(0, 0, 0, (0, 0), 0x50012);
        cds.start_spell(75, &auto_shot, 3200, t0);
        let mid = t0 + Duration::from_millis(100);
        assert_eq!(
            cds.info(75, 0, Some(&auto_shot), mid).remaining_ms,
            0,
            "no sweep for a category-0 record"
        );
        assert!(
            !cds.not_ready(75, 0, Some(&auto_shot), mid),
            "no local refusal either"
        );
    }
}
