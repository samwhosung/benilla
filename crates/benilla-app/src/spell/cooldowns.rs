//! The player's cooldown store — a mirror of the client's `SpellHistory` list (decision 0137
//! phase 4). Every law here is the client's own mechanism (the SPELLHISTORY node ops
//! `0x6e12c0`/`0x6e13e0`/`0x6e1630`/`0x6e1790`,
//! `StartCooldown 0x6e2c60`, `StartGlobalCooldown 0x6e2de0`, and the SMSG handlers
//! `0x6e9460`/`0x6e95d0`/`0x6e9670`/`0x6e9730`), transcribed onto `Instant`/`Duration`:
//!
//! - A **record** carries three independent timer pairs, exactly the SPELLHISTORY fields: the
//!   spell's own recovery, its category's shared recovery, and the global-cooldown pair
//!   (`startRecoveryCategory`/`startRecoveryTime`). `on_hold` parks the first two until
//!   `SMSG_COOLDOWN_EVENT` starts them (`SPELL_ATTR_COOLDOWN_ON_EVENT` — Stealth, Feign Death).
//! - The **read** ([`Cooldowns::info`], the client's `GetCooldownInfo 0x6e13e0`) resolves a
//!   queried spell against all three: nodes matching its id (+ cast item), nodes matching its
//!   category, and nodes whose GCD category matches its `startRecoveryCategory` — the mechanism
//!   that spreads one cast's GCD onto every other button. The longest remaining wins.
//! - **Who starts what**: the GCD starts locally at
//!   cast-send (`0x6e58fb`); the spell's own recovery is client-computed from `Spell.dbc` and
//!   inserted when **our own `SMSG_SPELL_GO`** arrives (`HandleSpellGo`'s self-insert tail
//!   `0x6e8498`/`0x6e8566`, anchored at the receive-time, onHold from Attributes bit 25);
//!   `SMSG_SPELL_COOLDOWN` is the server *override/refresh* path (school lockouts, pet lists) —
//!   vmangos sends no packet for a plain cast's cooldown. A failed cast (`SMSG_CAST_RESULT`)
//!   never reached its GO, so the fail path clears only the GCD (`0x6e1d83 → 0x6e1630`).
//!
//! The store is generation-counted: every mutation bumps [`Cooldowns::generation`], and the UI
//! feed fires `ACTIONBAR_UPDATE_COOLDOWN` on the change — natural *expiry* bumps nothing (the
//! widget animates itself from `(start, duration)` and hides at the end, the reference
//! `Cooldown.lua` machine).

use std::time::{Duration, Instant};

use bevy::prelude::*;

use benilla_formats::SpellDisplay;
use benilla_protocol::messages::ItemUseSpell;

/// One timer pair: when it started and how long it runs. Zero-duration = not tracked.
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

/// One SPELLHISTORY record (`0x6e12c0`'s node, byte-for-byte in
/// spirit: spellID/itemID/recovery pair/category+pair/onHold/GCD pair).
#[derive(Clone, Debug)]
struct Record {
    spell_id: u32,
    /// The cast item's template entry (`0` = a plain spell record) — item-use cooldowns key on
    /// the pair, the client's `[eax+8]==spellId && [eax+0xc]==itemID` match.
    item_id: u32,
    recovery: Timer,
    category: u32,
    /// Whether [`Self::category`]'s SpellCategory row carries the flags-bit-`0x2` wildcard —
    /// the category leg then contributes to EVERY query (`0x6e13e0` @ `6e1563`; wand Shoot's
    /// 351 is the only 5875 carrier). Resolved at catalog load, copied here at insert.
    category_wildcard: bool,
    category_recovery: Timer,
    /// Parked until `SMSG_COOLDOWN_EVENT` (`SPELL_ATTR_COOLDOWN_ON_EVENT`): the recovery pairs
    /// hold their *durations* but their clocks haven't started.
    on_hold: bool,
    gcd_category: u32,
    gcd: Timer,
}

/// What one queried action's cooldown reads (`GetActionCooldown`'s triple, app-side): the
/// winning timer's **absolute start** + full duration + time remaining, and whether it is
/// actually running (`enabled == false` = an on-hold record — the reference API's `enable == 0`,
/// which the `CooldownFrame_SetTimer` law hides). Carrying the start is the reference's own
/// convention (`GetCooldownInfo 0x6e13e0` returns the record's start, never a "remaining"), and
/// it is what makes the read re-arm-proof: two arms of a same-length cooldown can never alias,
/// because their starts differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CooldownInfo {
    /// When the winning timer started (for an on-hold record: when it was inserted).
    pub start: Instant,
    pub remaining_ms: u32,
    pub duration_ms: u32,
    pub enabled: bool,
}

impl CooldownInfo {
    /// The pushable UI triple `(start_ms on the GetTime clock, duration_ms, enabled)`, or `None`
    /// when cold. `anchor`/`ui_now` are the frame's ATOMIC clock pair
    /// ([`crate::ui_script::UiClock`]) — the `Instant` whose deltas the VM clock accumulates,
    /// beside the value it accumulated to — so the subtraction maps the start across without
    /// either side knowing the other's epoch, and derives the SAME whole-ms number every frame
    /// for one arm (both legs advance in lockstep by construction). A RE-ARM always derives a
    /// new one — the property the old `(remaining, duration)` shape lacked (two arms of the same
    /// cooldown read byte-identical and the seam kept the first, elapsed, anchor: the
    /// vanished-GCD-pie bug). Never convert through a locally sampled `Instant::now()`: that
    /// re-measures the tick→caller scheduling gap every frame and wobbles the derived start by
    /// the jitter (±12 ms observed live), turning a running cooldown into a per-frame "changed"
    /// triple.
    pub(crate) fn ui_triple(&self, anchor: Instant, ui_now: f64) -> Option<(i64, u32, bool)> {
        (self.remaining_ms > 0).then(|| {
            // Signed both ways: a timer armed AFTER the anchor sample (mid-frame, before the
            // next tick) projects forward, so its first-frame derivation already equals every
            // later frame's.
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

/// The player's cooldown list (the client's self `SpellHistory` @0xcecaec; the pet list has no
/// benilla consumer). One resource, written by the net bridge + the cast-send path, read by the
/// action-bar feed.
#[derive(Resource, Default)]
pub(crate) struct Cooldowns {
    records: Vec<Record>,
    /// Bumped on every mutation — the UI feed's `ACTIONBAR_UPDATE_COOLDOWN` edge.
    pub(crate) generation: u64,
    /// Bumped when [`Self::prune`] actually removes records — the **natural-expiry** edge, kept
    /// apart from [`Self::generation`] on purpose: a pruned record flips a
    /// pushed `ui_triple` to `None`, so a feed gated on this store must reopen at that instant —
    /// but the action feed's `ACTIONBAR_UPDATE_COOLDOWN` must NOT fire there, because the
    /// reference sweeps elapsed records on events and never announces a natural expiry.
    expiry_epoch: u64,
}

impl Cooldowns {
    fn bump(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    /// The single counter a gated feed watches: moves exactly when a snapshot
    /// built over this store could differ — on any mutation, or on a natural expiry the prune
    /// noticed. (Both terms only ever increment, so the sum moves whenever either does.)
    pub(crate) fn feed_epoch(&self) -> u64 {
        self.generation.wrapping_add(self.expiry_epoch)
    }

    /// True while an elapsed record still awaits its prune — the one window where a snapshot
    /// built over this store changes with NO counter having moved yet: the feeds run before
    /// `feed_action_state`'s per-frame prune, so on the exact frame a timer crosses zero the
    /// pushed triple flips to `None` a frame ahead of [`Self::feed_epoch`].
    /// The predicate is prune's own removal test; cold and parked it costs an empty iteration.
    pub(crate) fn sweep_pending(&self, now: Instant) -> bool {
        self.records.iter().any(|r| {
            !r.on_hold
                && r.recovery.remaining(now).is_zero()
                && r.category_recovery.remaining(now).is_zero()
                && r.gcd.remaining(now).is_zero()
        })
    }

    /// The insert primitive (`AddCooldown 0x6e12c0`): nothing to track → no-op; else **append a
    /// new record**. The client never matches-by-id here — its "reuse" scan is free-list node
    /// recycling, an allocator detail (no `[node+8]==spellId` compare anywhere in `0x6e12c0`'s
    /// body), where every other op in the family explicitly walks
    /// "each node matching id". So one spell can hold SEVERAL records at once — and must: the
    /// cast-send GCD arm (`StartGlobalCooldown`, a gcd-only node) and the GO self-insert
    /// (`StartCooldown`, which passes `gcd=0,0`) are separate nodes. The find-and-replace this
    /// used to do made the GO insert of any cooldown-carrying spell (Frost Nova) overwrite its
    /// own running GCD node — the whole bar's pie flashed and died ~100 ms after the press,
    /// while cooldown-less spells (Arcane Explosion) kept theirs.
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
        // The client's early-out: no recovery, no category recovery, not on hold, no GCD —
        // nothing to track (6e12c3).
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

    /// Prune records with nothing left to say: every timer elapsed and not on hold. Behaviorally
    /// invisible (an elapsed record contributes zero remaining) — bounds the list without the
    /// client's event-driven sweep sites. A removal bumps [`Self::expiry_epoch`], not
    /// [`Self::generation`]: gated feeds must see the expiry, the event edge must not.
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

    /// Start a spell's own cooldown (`StartCooldown 0x6e2c60`, spell-only path): recovery /
    /// category from `Spell.dbc`, on-hold when the spell is `SPELL_ATTR_COOLDOWN_ON_EVENT`.
    /// No GCD here — that's [`Self::start_gcd`]'s separate insert.
    ///
    /// `ranged_attack_time_ms` is the ranged-shot pad (the category scaler `0x6e2b60`'s
    /// `add [categoryRecoveryTime], [player+0x110]+0x1e8`): the caster's live
    /// `UNIT_FIELD_RANGEDATTACKTIME`
    /// when [`SpellDisplay::ranged_speed_cooldown`], else 0. It folds into the CATEGORY timer —
    /// the Throw/wand-Shoot sweep with all-zero DBC recovery — and rides the insert even for
    /// category 0 (Auto Shot), where no read surfaces it (the client's SpellCategory[0]-is-NULL
    /// asymmetry: Auto Shot never sweeps).
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

    /// Start an item use's cooldown (`StartCooldown 0x6e2c60` with an item record): the wire's
    /// server-resolved triple, each negative falling back to the spell's own `Spell.dbc` value
    /// (the client's `>= 0` pick on the item slots).
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
            // The wildcard mark rides the CATEGORY; the wire's per-slot category resolves it
            // through the spell only when the two agree (no 5875 item carries the one wildcard
            // category, wand Shoot's 351 — a named corner, not a live one).
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

    /// Arm the global cooldown at cast-send (`StartGlobalCooldown 0x6e2de0` ← the cast-send arm
    /// `0x6e58fb`, shared by all three commit wires — CAST_SPELL, USE_ITEM, PET_CAST — keyed on
    /// the cast SPELL): arms iff `startRecoveryTime != 0` (the byte predicate `6e2e0f–6e2e3d`:
    /// enter on either pair field nonzero, then bail on a zero post-mod time — so a
    /// `{cat≠0, time=0}` spell arms nothing; Attack, Auto Shot and wand Shoot carry (0,0)).
    /// `onHold` rides Attributes bit 25 exactly as the ref passes it (`([rec+0x18]>>0x19)&1`).
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

    /// Clear only the GCD fields of a spell's record(s) (`0x6e1630` — the cast-fail path): a
    /// rejected cast opens the global cooldown again immediately.
    pub(crate) fn clear_gcd(&mut self, spell_id: u32, now: Instant) {
        let mut touched = false;
        for r in &mut self.records {
            if r.spell_id == spell_id && !r.gcd.duration.is_zero() {
                // `0x6e1630` zeroes BOTH fields (+0x28 startRecoveryCategory, +0x2c time).
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

    /// `SMSG_COOLDOWN_EVENT` (`0x6e1790`, force=0): an on-hold record's parked timers start
    /// **now**; a running record is left alone.
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

    /// `SMSG_CLEAR_COOLDOWN` / the cast-fail revert (`0x6e1790`, force=1): remove the spell's
    /// record(s) outright.
    pub(crate) fn clear_spell(&mut self, spell_id: u32) {
        let before = self.records.len();
        self.records.retain(|r| r.spell_id != spell_id);
        if self.records.len() != before {
            self.bump();
        }
    }

    /// `SMSG_COOLDOWN_CHEAT` (`0x6e9700`): drain the whole list.
    pub(crate) fn wipe(&mut self) {
        if !self.records.is_empty() {
            self.records.clear();
            self.bump();
        }
    }

    /// **The list belongs to the world session, not to the process** — the
    /// session-end clear every other net-backed store already had
    /// ([`crate::net::session::disconnected`]).
    ///
    /// `SMSG_INITIAL_SPELLS` carries the WHOLE set of cooldowns still running, at every world
    /// entry, and [`Self::seed_initial`] appends (the reference's `AddCooldown 0x6e12c0` never
    /// matches by id — see [`Self::add`]). So a store that survives the socket answers the *old*
    /// session's records for the rest of their lives: a second login on the same character reads
    /// its own stale copy rather than the wire's fresh remainder (measured live — a 600 s record
    /// re-derived from the first login still winning over the second login's 594 s one), and a
    /// login on a DIFFERENT character inherits cooldowns that are not theirs, for spells they may
    /// not know. Nothing is lost by clearing: the next world entry — including 0065's seamless
    /// reconnect, which is a full re-login — re-seeds from the wire.
    pub(crate) fn clear_session(&mut self) {
        self.wipe();
    }

    /// One `SMSG_SPELL_COOLDOWN` pair (`0x6e9460`'s per-entry law): a nonzero wire duration is
    /// the spell recovery verbatim (category untracked); zero means "the spell's own Spell.dbc
    /// recovery + category recovery". `SPELL_ATTR_COOLDOWN_ON_EVENT` parks it and suppresses the
    /// GCD pair; otherwise the spell's GCD pair rides along.
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

    /// `SMSG_ITEM_COOLDOWN` (`0x6e95d0`): the fixed 30 000 ms use cooldown on the item's on-use
    /// spell — the 30 s is the client's hardcode, nothing else rides the wire.
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

    /// One `SMSG_INITIAL_SPELLS` cooldown entry: the wire carries **remaining** ms (vmangos
    /// computes them at send), so the record starts now and runs that remainder — the client
    /// can't know the original start either. A *permanent* cooldown (`spell_cd_ms == 1`, the
    /// category word's top bit) re-arms server-side; its 1 ms is carried verbatim (harmless — the
    /// server refuses the cast regardless).
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
            // No display in reach here; the one wildcard category (wand Shoot's 351) never
            // arrives via the initial-cooldowns list on 1.12 data — a named, dead corner.
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

    /// One `SMSG_PET_SPELLS` cooldown entry — [`Self::seed_initial`]'s pet twin,
    /// separate because the pet block's ids are `u32` where the player's login list packs them
    /// into `u16`, and because its category duration carries a marker bit the player's does not.
    ///
    /// Both remainders are what is LEFT (vmangos computes them at send, `WritePetSpellsCooldown`),
    /// so the record starts now and runs the remainder. The category word's
    /// [`PET_COOLDOWN_PERMANENT`] bit marks a server-re-armed cooldown; it is stripped, because
    /// carrying it would read as a ~37-hour sweep on the button.
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
            // The wildcard row's flag comes off the catalog when we have it — the same resolve
            // `start_spell` does. Absent catalog ⇒ false, the non-wildcard reading.
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

    /// The client's `IsSpellOnCooldown 0x6e1690` — an **"has an on-hold (not-yet-started)
    /// record"** predicate, NOT a general on-cooldown test (both legs return 1 only when the
    /// matched node's onHold byte is
    /// set; `+0x28`/`+0x2c` are never referenced and no time source is called). Its reference
    /// consumers are all bit25/cooldown-on-event gates: the usable walk's grey-while-parked leg
    /// (`0x6e3fb1`) — ours — and the cast-fail on-hold revert.
    ///
    /// **It is item-keyed, and that is not cosmetic**. The node walk's spell-id leg is
    /// `node+0x08 == spellId && node+0x0c == itemId`
    /// (`6e173f`/`6e1744`), and `0x6e2fc0` — the action bar's ITEM gate, and the **sole** consumer
    /// of the item-keyed form image-wide — passes the item ENTRY as that second argument
    /// (`6e3037 push esi`). Every other caller passes `0` (`0x6e2fa0`'s `push 0` at `6e2fa9`).
    /// So an item's own parked record is only findable under its entry; querying a spell slot's
    /// `(spell, 0)` form can never see it.
    ///
    /// `category` is the caller's for the same reason: `0x6e1690` starts from the spell's own
    /// `[SpellRec+0x8]` and, when `itemId != 0`, **overwrites** it from the item's fifth spell
    /// array `spellcategory[5]` (`rec+0x16c+4i`, `6e16f6`–`6e171b`, last match wins). Our wire
    /// hands that resolved value over as `ItemUseSpell::category`, so the item's call site passes
    /// the item's category and a spell's passes the spell's.
    pub(crate) fn has_on_hold_record(&self, spell_id: u32, item_entry: u32, category: u32) -> bool {
        self.records.iter().any(|r| {
            r.on_hold
                && ((r.spell_id == spell_id && r.item_id == item_entry)
                    || (category != 0
                        && r.category == category
                        && !r.category_recovery.duration.is_zero()))
        })
    }

    /// The cast validator's FIRST rung (`0x6094f0` @ `0x609565` → `0x6e2ea0` → the getter): a
    /// press is refused "not ready" iff [`Self::info`] reads ANY remaining — the spell's own
    /// pair, a category match, or the GCD leg, one query (which closed 0379's
    /// INTERIM). The GCD refusal predicate is therefore the GETTER's:
    /// `pressed.startRecoveryCategory == node.startRecoveryCategory && node.time != 0` — the
    /// pressed spell's own `startRecoveryTime` is never consulted (a `{cat≠0, time=0}` press —
    /// the scroll spells — IS refused during the GCD), and Attack / profession presses can never
    /// be refused (the getter's head exclusion). The item fork (`0x60952b`) is the CALLER's:
    /// an item press queries `(use_spell, item_entry)` and refuses 0x28; a spell press queries
    /// `(spell, 0)` and refuses 0x3c. Refusing locally is what keeps the server's NOT_READY fail
    /// — whose faithful revert [`Self::clear_gcd`] wipes the RUNNING GCD — off the wire (the
    /// 0379 spam-press vanished-pie loop).
    pub(crate) fn not_ready(
        &self,
        spell_id: u32,
        item_entry: u32,
        spell: Option<&SpellDisplay>,
        now: Instant,
    ) -> bool {
        self.info(spell_id, item_entry, spell, now).remaining_ms > 0
    }

    /// The per-spell read (`GetCooldownInfo 0x6e13e0`, the complete match law): resolve
    /// `spell_id` (as cast from `item_entry`, `0` for a plain
    /// spell) against EVERY record, three legs each, **longest remaining wins**:
    ///
    /// - **head exclusion**: `Effect[0] ∈ {ATTACK, TRADE_SKILL}` reads cold unconditionally
    ///   (`6e1439`/`6e1442`) — no pie and no refusal for the Attack/profession buttons, ever;
    /// - **spell-id leg**: id+item match; a parked record reads full duration with
    ///   `enabled == false` (the `CooldownFrame_SetTimer` law hides it);
    /// - **category leg**: node-category equality — or ANY query when the node's category is a
    ///   flags-bit-`0x2` wildcard row (wand Shoot's 351: the whole-bar swing sweep). A parked
    ///   record contributes its full duration, disabled (ref: start=now while onHold);
    /// - **GCD leg** (`6e15cc`): plain equality `node.gcd_category == queried
    ///   startRecoveryCategory` with the NODE's time nonzero — the queried spell's own
    ///   `startRecoveryTime` plays no role, and the leg ignores onHold and never disables.
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
                    // Parked: full duration remaining, not running (enable == 0 hides the sweep).
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

        // The cast spell itself and a DIFFERENT spell with the same startRecoveryCategory both
        // read the GCD; Charge (no GCD pair) reads nothing.
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

        // …and the press gate reads it — the ONE getter is the refusal (0948 closed
        // 0379's INTERIM: there is no separate GCD site); the corrected `0x6e1690` on-hold
        // predicate stays false (no parked record).
        assert!(cds.not_ready(133, 0, Some(&fireball()), mid));
        assert!(!cds.has_on_hold_record(133, 0, fireball().category));
    }

    /// The director's Frost Nova report: a cooldown-carrying spell's GO
    /// self-insert (`StartCooldown` passes `gcd=0,0`) lands on its OWN node and must never eat
    /// the cast-send GCD node — the bar's pie flashed and died ~100 ms after every Frost Nova
    /// press while cooldown-less spells (Arcane Explosion) kept theirs, because `add` used to
    /// find-and-replace by `(spell, item)`.
    #[test]
    fn a_go_self_insert_never_wipes_the_running_gcd() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        // Frost-Nova-shaped: own 25 s recovery, category 35, the ordinary 133/1500 GCD pair.
        let frost_nova = spell(35, 25_000, 0, (133, 1500), 0);
        cds.start_gcd(122, &frost_nova, t0); // the cast-send arm
        cds.start_spell(122, &frost_nova, 0, t0 + Duration::from_millis(100)); // the GO insert

        let mid = t0 + Duration::from_millis(200);
        // Every GCD-carrying sibling still reads the RUNNING GCD…
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
        // …and Frost Nova's own button reads its own cooldown (longest remaining wins).
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

        // The 0x6e1a00 fail path: GCD cleared (0x6e1630) + the record force-removed (0x6e3050).
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
        // A different spell in category 44 reads the shared remainder…
        let sibling = spell(44, 0, 15_000, (0, 0), 0);
        let s = cds.info(999, 0, Some(&sibling), mid);
        assert_eq!((s.remaining_ms, s.duration_ms), (10_000, 15_000));
        assert!(
            cds.not_ready(999, 0, Some(&sibling), mid),
            "category lock is a not-ready"
        );
        // …an unrelated spell reads nothing.
        assert_eq!(cds.info(133, 0, Some(&fireball()), mid).remaining_ms, 0);
    }

    #[test]
    fn an_on_event_cooldown_parks_until_the_event_starts_it() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        // Feign Death: 30 s recovery, SPELL_ATTR_COOLDOWN_ON_EVENT (bit 25).
        let fd = spell(0, 30_000, 0, (0, 0), 0x0200_0000);
        cds.start_spell(5384, &fd, 0, t0);

        // Parked: full duration, enabled == false (the sweep is hidden), but "not ready" holds.
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

        // SMSG_COOLDOWN_EVENT starts the clocks NOW.
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

    /// **The session end empties the list, so the next login's wire is the whole truth**.
    /// `seed_initial` appends by design (the reference's `AddCooldown` never
    /// matches by id), so without the clear the previous session's record stays and — being
    /// stamped with the FULL remainder it had at the earlier login — outlives and outbids the
    /// fresh one. Measured live before the fix: a second world entry read `d=600` where the wire
    /// had just said 594.
    #[test]
    fn a_session_end_empties_the_list_so_the_next_logins_wire_is_the_whole_truth() {
        use benilla_protocol::messages::SpellCooldown;
        let wire = |ms| SpellCooldown {
            spell_id: 12975, // Last Stand — 10 minutes, no category
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

        // …the player logs out six seconds later and comes straight back in; the server's list
        // now says 594 s.
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

    /// **`0x6e1690`'s item-keyed form**. The node
    /// walk's spell-id leg is `node+0x08 == spellId && node+0x0c == itemId`, and the action bar's
    /// ITEM gate `0x6e2fc0` is the sole caller image-wide that passes a **non-zero** `itemId` —
    /// the item ENTRY, at `6e3037 push esi`. Our store keys an item's record `(use_spell, entry)`
    /// to match, so the spell-slot form `(spell, 0)` must NOT find it: a query keyed `0` walking
    /// into an item's parked record would be the leg firing for the wrong button, and a query
    /// keyed on the entry failing to find it is the bug this pins — the item's gate would have
    /// rested on the category leg alone.
    #[test]
    fn an_on_hold_record_is_findable_only_under_the_key_that_armed_it() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        // An on-use item whose spell carries bit 25 (cooldown-on-event), category 0 so the
        // category leg cannot mask the spell-id leg's answer either way.
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
        // A potion: the wire triple resolved category 4 / 60 s, use-cooldown "the spell's own"
        // (-1 → the spell has none).
        let use_spell = ItemUseSpell {
            spell_id: 439,
            cooldown_ms: -1,
            category: 4,
            category_cooldown_ms: 60_000,
        };
        let potion_spell = spell(4, 0, 60_000, (133, 1500), 0);
        cds.start_item(118, &use_spell, Some(&potion_spell), t0);

        let mid = t0 + Duration::from_secs(15);
        // The action-bar read for the potion action (spell 439 as cast from item 118) sees the
        // category remainder…
        let info = cds.info(439, 118, Some(&potion_spell), mid);
        assert_eq!((info.remaining_ms, info.duration_ms), (45_000, 60_000));
        // …and so does any other category-4 potion.
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

        // After the GCD (1.5 s) but inside Charge's 15 s: only the GCD record goes.
        cds.prune(t0 + Duration::from_secs(5));
        assert_eq!(cds.records.len(), 1);
        assert_eq!(cds.records[0].spell_id, 100);

        cds.prune(t0 + Duration::from_secs(20));
        assert!(cds.records.is_empty());
    }

    /// The gated feeds' counter (1439): `feed_epoch` moves on a mutation AND on a natural
    /// expiry the prune noticed — but a prune that removes nothing moves nothing, or the
    /// per-frame prune call would hold every cooldown-watching gate open forever.
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

    /// The vanished-GCD-pie regression (spam-press: fail-clear + re-arm inside one inter-feed
    /// gap): the UI triple must be (a) frame-stable for one running cooldown — re-reading the
    /// same arm later yields the same start — and (b) distinct across two arms, so the seam can
    /// never mistake a fresh GCD for the elapsed one it replaced.
    #[test]
    fn the_ui_triple_is_stable_per_arm_and_distinct_across_arms() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        cds.start_gcd(772, &fireball(), t0); // press #1 arms the GCD

        // Two reads of the SAME arm, frames apart, on both clocks in lockstep → identical triple.
        let read1 = cds
            .info(772, 0, Some(&fireball()), t0 + Duration::from_millis(16))
            .ui_triple(t0 + Duration::from_millis(16), 10.016);
        let read2 = cds
            .info(772, 0, Some(&fireball()), t0 + Duration::from_millis(160))
            .ui_triple(t0 + Duration::from_millis(160), 10.160);
        assert_eq!(read1, Some((10_000, 1500, true)));
        assert_eq!(read1, read2, "one arm reads one start, every frame");

        // The spam cycle: the fail clears the GCD, the re-press re-arms 200 ms later — the feed
        // never observes the cleared gap, but the fresh arm carries a fresh start regardless.
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

    /// The conversion is signed both ways: a timer armed AFTER the frame's clock-pair sample
    /// (mid-frame — the anchor predates the arm until the next tick) must project the SAME
    /// start the next frame's pair re-derives, so frame one never pushes a value frame two
    /// then "corrects" (a phantom re-arm at every cast).
    #[test]
    fn a_mid_frame_arm_derives_the_same_start_as_the_next_frames_pair() {
        let anchor0 = Instant::now();
        let mut cds = Cooldowns::default();
        // Armed 4 ms after this frame's anchor sample.
        cds.start_gcd(133, &fireball(), anchor0 + Duration::from_millis(4));

        // Frame 1 converts through the pre-arm pair; frame 2 through the next tick's pair
        // (both legs advanced by the same 16 ms). One arm — one start, both frames.
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

    /// The GCD refusal predicate, corrected by 0948 (the getter's GCD leg `6e15cc`): a press
    /// is refused iff its `startRecoveryCategory` EQUALS the armed node's (node time ≠ 0) — the
    /// pressed spell's own `startRecoveryTime` is never consulted. So a `{133, 0}` press (the
    /// scroll spells) IS refused during the GCD (the old predicate passed it), a `{0, 0}`
    /// press (Attack shape, Charge) flows, and the lock lifts at expiry.
    #[test]
    fn a_running_gcd_locks_presses_by_category_equality_alone() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        assert!(
            !cds.not_ready(133, 0, Some(&fireball()), t0),
            "no GCD running — nothing locks"
        );

        cds.start_gcd(772, &fireball(), t0); // the successful cast arms the 133/1500 GCD
        let mid = t0 + Duration::from_millis(200);
        assert!(
            cds.not_ready(133, 0, Some(&fireball()), mid),
            "the spam press 200 ms later is locked — refused, never sent, the GCD lives"
        );
        // A {133, 0} press — Scroll of Armor's shape — is REFUSED: the node's category matches
        // and only the NODE's time matters (the corrected divergence).
        let scroll = spell(0, 0, 0, (133, 0), 0x10000);
        assert!(
            cds.not_ready(8091, 0, Some(&scroll), mid),
            "a zero-GCD press in the shared category is locked on the reference"
        );
        // Charge (no GCD pair at all) is untouched.
        assert!(!cds.not_ready(100, 0, Some(&charge()), mid));
        // The lock lifts exactly when the GCD elapses.
        assert!(!cds.not_ready(133, 0, Some(&fireball()), t0 + Duration::from_millis(1_501)));
    }

    /// The getter's HEAD exclusion (`6e1439`/`6e1442`): Attack / profession presses can never
    /// be cooldown-refused and their buttons never read a pie — whatever is running.
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

    /// The category-wildcard leg (`6e1563`: a SpellCategory row with flags bit 0x2 matches EVERY
    /// query — wand Shoot's 351, the only 5875 carrier): a running wildcard-category cooldown
    /// sweeps and not-readies every button, the whole-bar wand-swing feel.
    #[test]
    fn a_wildcard_category_record_reaches_every_query() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        // Shoot-shaped: category 351 (wildcard), all-zero DBC recovery, the ranged pad supplies
        // the category duration (decision 0378's insert shape).
        let shoot = SpellDisplay {
            category: 351,
            category_wildcard: true,
            attributes: 0x50012,
            ..Default::default()
        };
        cds.start_spell(5019, &shoot, 1500, t0);
        let mid = t0 + Duration::from_millis(500);
        // Fireball (category 0, unrelated) reads the wand swing…
        let fb = cds.info(133, 0, Some(&fireball()), mid);
        assert_eq!((fb.remaining_ms, fb.duration_ms), (1000, 1500));
        // …and is locally not-ready for its duration.
        assert!(cds.not_ready(133, 0, Some(&fireball()), mid));
        assert!(!cds.not_ready(133, 0, Some(&fireball()), t0 + Duration::from_millis(1_501)));
    }

    #[test]
    fn cheat_wipe_and_clear_bump_the_generation_only_on_change() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        let g0 = cds.generation;
        cds.clear_spell(133); // nothing tracked — no bump
        cds.wipe();
        assert_eq!(cds.generation, g0);

        cds.start_gcd(133, &fireball(), t0);
        assert_ne!(cds.generation, g0);
        let g1 = cds.generation;
        cds.wipe();
        assert_ne!(cds.generation, g1);
        assert_eq!(cds.info(133, 0, Some(&fireball()), t0).remaining_ms, 0);
    }

    /// The ranged-shot pad (`0x6e2b60`): a Throw-shaped
    /// spell (category 76, all-zero DBC recovery) sweeps the weapon's attack time via its
    /// CATEGORY timer, and refuses a recast within it.
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

    /// Auto Shot (category 0) inserts the same padded timer but NO read surfaces it — the
    /// client's `SpellCategory[0]`-is-NULL asymmetry: no sweep, no local recast refusal.
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
