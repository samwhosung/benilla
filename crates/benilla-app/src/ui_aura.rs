//! The app-side **aura feed** (decisions 0255/0257): turns the live `UNIT_FIELD_AURA` blocks of
//! our own avatar and of the current target into the ordered [`AuraState`] lists the `UnitAura`
//! bindings read, maintains the reference client's insertion-ordered display cache for the player,
//! joins in the self-only durations, and drains the right-click cancels back to the wire.
//!
//! Two things make this more than a snapshot projection:
//!
//! - **Order is state.** The player's buff bar draws a densely packed cache in *insertion* order that
//!   repacks on removal (`PlayerAuras_Update`, byte-verified — decision 0257). Ascending slot order,
//!   which a fresh descriptor read would give, is a *different* order and would shuffle icons into
//!   recycled slots. So [`PlayerAuraCache`] is carried across frames: survivors keep their position,
//!   a dropped aura closes its gap, a new aura appends at the end.
//! - **Durations arrive out of band, and before the slot is named.** `SMSG_UPDATE_AURA_DURATION`
//!   ([`AuraDurations`], filled by the net apply path) is keyed by raw slot and lands *before* the
//!   `UNIT_FIELD_AURA` delta that says which spell sits there (verified, decision 0257 B6; measured
//!   at one frame, ~50 ms, on a fresh apply — decision 0846). So a stamp's slot is **empty at the
//!   moment it arrives**, and the only thing that may invalidate it is the aura it would be joined
//!   to, never the slot's momentary occupancy: the feed keeps every stamp and gates the *join* on
//!   the aura having appeared no earlier than the packet. That gate is what stops a timer left by a
//!   since-expired occupant showing on the permanent aura that recycled its slot (the reference
//!   avoids this via a DBC "until cancelled" flag we don't parse — decision 0257 §3).
//!
//! And one thing the bar does **not** get from here: the running countdown. Rust recomputes each
//! aura's `expirationTime` every frame, but the event this feed fires is the reference's
//! discrete-change edge (`PLAYER_AURAS_CHANGED`), which a duration refresh does not trip. The
//! reference resolves that the same way — `BuffButton_OnUpdate` re-reads `GetPlayerBuffTimeLeft`
//! **every frame** and caches only the buff *index* on the event — and so does `BuffFrame.xml`.
//! A bar that cached the expiry on the event instead is decision 0846's second defect.
//!
//! Scope: the **local player** (decisions 0255/0257) and the **target** (the target frame's aura
//! rows — 0255's deferred slice). The target's list is the byte-verified *other-unit* law
//! (decision 0257): `UnitBuff`/`UnitDebuff` on another unit read that unit's own `UNIT_FIELD_AURA`
//! straight, **ascending raw slot within the half** — no insertion cache, no durations (the 1.12
//! wire carries none for anyone but yourself). It **does** carry the display filter, though: an aura
//! whose spell is flagged never-display (`NO_AURA_ICON`/`DO_NOT_DISPLAY` — a warrior stance) is
//! hidden on *every* aura display, target rows included, not just the player's own bar (decision
//! 0417, correcting 0268's scope note — the director watched the reference hide a
//! target's Battle Stance, which the "other-unit reads straight" reading can't explain: `NO_AURA_ICON`
//! means exactly that, everywhere). A self-target is the one exception in *ordering*: decision 0257 §2
//! resolves the Era API's single `UnitAura` toward the player-bar law under every token, so targeting
//! yourself shows the same insertion-ordered list — filtered identically either way.

use std::collections::HashMap;

use bevy::prelude::*;

use benilla_formats::SpellCatalog;
use benilla_protocol::messages::{UnitAuraSlot, AURA_FLAG_CANCELABLE, UNIT_AURA_POSITIVE_SLOTS};
use benilla_ui::script::{AuraState, ScriptValue, TrackingState, UiScript};

use crate::char_select::ClientState;
use crate::net::{
    ClientCommand, Guid, GuidIndex, NetCommands, ObjectStore, Reputations, SelfPlayer,
};
use crate::target::{can_assist, Factions, Selection};
use crate::ui_action::Spells;
use crate::ui_script::UiInput;
use crate::ui_unit::UnitFeed;

/// The self-only aura durations, keyed by raw `UNIT_FIELD_AURA` slot — the decoded
/// `SMSG_UPDATE_AURA_DURATION` payload, stamped with the real time it arrived. Written by the net
/// apply path (which owns the event stream), read by [`feed_auras`]. A slot's entry is overwritten
/// by each fresh packet (apply/refresh) and dropped only when the *session* ends (leaving
/// `InWorld`) — never per-frame on slot occupancy, which would delete every stamp in the frame
/// before its own aura arrives (decision 0846), and never on the avatar entity's absence, which a
/// cross-map worldport produces mid-session while the auras themselves live on (decision 0900).
/// This is the reference's `0xbc5f68` expiry array: raw-slot-keyed, written only by the duration
/// packet, and a process global that no world change touches. Bounded by construction: one entry
/// per raw slot, ≤ 48.
#[derive(Resource, Default)]
pub(crate) struct AuraDurations {
    by_slot: HashMap<u8, DurationStamp>,
}

struct DurationStamp {
    /// The span in seconds the last packet reported. On an apply or a refresh this **is** the
    /// aura's full duration (vmangos sends `GetAuraDuration()` right after setting it), which is
    /// what `UnitBuff`'s Era-shaped `duration` return wants. On a re-send it is only the
    /// *remaining* — `Map::Add` replays every holder's current duration at world entry, and cast
    /// pushback (`DelaySpellAuraHolder`) reports a shortened one. The 1.12 wire carries no maximum
    /// at all, for anyone, so there is nothing better to report; the buff bar draws
    /// [`Self::expires_at`] and never this.
    total: f64,
    /// The real-clock instant the aura runs out (`received_at + total`). Real, never virtual: see
    /// the net apply path's aura tuple — a server-sent span counts down in real seconds.
    expires_at: f64,
    /// The real-clock instant the packet arrived — the freshness gate against a recycled slot.
    received_at: f64,
}

impl AuraDurations {
    /// Record a `SMSG_UPDATE_AURA_DURATION`. Called from the net apply drain (decision 0257).
    pub(crate) fn set(&mut self, slot: u8, remaining_ms: u32, now: f64) {
        let total = f64::from(remaining_ms) / 1000.0;
        if trace_period().is_some() {
            info!(
                "aura trace: SMSG_UPDATE_AURA_DURATION slot {slot} = {remaining_ms} ms @ {now:.2}"
            );
        }
        self.by_slot.insert(
            slot,
            DurationStamp {
                total,
                expires_at: now + total,
                received_at: now,
            },
        );
    }
}

/// One aura in the player's display cache — a benilla mirror of a `0xbc6040` record (decision 0257).
/// Kept across frames so the insertion order survives; the display fields refresh from the
/// descriptor each frame, the position does not.
struct CachedAura {
    slot: u8,
    spell_id: u32,
    /// The real-clock instant this aura first entered the cache — the duration freshness gate.
    appeared_at: f64,
    flags: u8,
    level: u8,
    stacks: u8,
}

/// The player's insertion-ordered aura cache (decision 0257): buffs and debuffs interleaved in the
/// order the reference client's `PlayerAuras_Update` would hold them. Split into the two filtered
/// lists only at the push, since the bindings filter by sign themselves.
#[derive(Resource, Default)]
pub(crate) struct PlayerAuraCache {
    auras: Vec<CachedAura>,
}

impl PlayerAuraCache {
    /// The live aura spell ids — `ui_tooltip`'s pre-feed reads these at arrival so a buff-bar
    /// hover's `SetPlayerBuff` view hits on the FIRST enter (the ask-once path stays as the
    /// odd-case fallback).
    pub(crate) fn spell_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.auras.iter().map(|a| a.spell_id)
    }
}

/// The aura-event cut: [`feed_auras`] fires `PLAYER_AURAS_CHANGED` / `UNIT_AURA("player")`
/// **synchronously** (`UiScript::fire_event` walks the handlers inline), so a feed whose pushed
/// state a handler re-reads on them runs `.before(AuraEvents)` — the stance feed, whose
/// `isActive` is the form aura's own slot and is pushed silently on that edge (decision 2009) —
/// or the handler reads last frame's state and the repaint waits for the next unrelated event.
/// The twin of [`crate::ui_action::CooldownEvents`].
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct AuraEvents;

/// Adds the aura feed. The `UnitAura` bindings live in `benilla-ui`; this supplies their data and
/// fires `UNIT_AURA`, and drains `CancelUnitBuff` back to the wire.
pub(crate) struct UiAuraPlugin;

impl Plugin for UiAuraPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AuraDurations>()
            .init_resource::<PlayerAuraCache>()
            .init_resource::<AuraFeedMemory>()
            // Feed before the VM dispatch (like the unit feed), so a frame's OnEvent sees the fresh
            // list; drain the cancels after, once the VM has queued them.
            .add_systems(Update, feed_auras.in_set(UnitFeed).in_set(AuraEvents))
            .add_systems(Update, drain_aura_cancels.after(UiInput))
            // The ONLY teardown of the aura state, and it hangs off the session edge — never off
            // the avatar entity's existence, which a worldport interrupts mid-session (0900).
            .add_systems(OnExit(ClientState::InWorld), end_session_aura_state);
    }
}

/// Reconcile the insertion-ordered cache against the live descriptor: drop entries whose slot no
/// longer holds their spell (expired, or the slot was recycled), then append newly occupied slots in
/// ascending order (so a same-tick multi-apply is deterministic). Survivors keep their position and
/// refresh their mutable fields. Mirrors `PlayerAuras_Update`'s shift-down + append (decision 0257).
fn reconcile(cache: &mut Vec<CachedAura>, live: &[UnitAuraSlot], now: f64) {
    cache.retain(|c| {
        live.iter()
            .any(|a| a.slot == c.slot && a.spell_id == c.spell_id)
    });
    for a in live {
        if let Some(c) = cache.iter_mut().find(|c| c.slot == a.slot) {
            // Same slot, same spell (retain guaranteed it) — refresh the volatile fields in place.
            c.flags = a.flags;
            c.level = a.level;
            c.stacks = a.stacks;
        } else {
            if trace_period().is_some() {
                info!(
                    "aura trace: slot {} took spell {} (descriptor delta) @ {now:.2}",
                    a.slot, a.spell_id
                );
            }
            cache.push(CachedAura {
                slot: a.slot,
                spell_id: a.spell_id,
                appeared_at: now,
                flags: a.flags,
                level: a.level,
                stacks: a.stacks,
            });
        }
    }
}

/// One aura's discrete projection (spell id, stacks, cancelable, dispel class — the fields a frame
/// repaints on). Excludes the countdown, which drifts every frame and is Lua's `OnUpdate` job, not
/// a `UNIT_AURA` trigger.
type AuraProjection = (u32, u8, bool, Option<String>);

/// The feed's edge-detection memory: per token, the projection of the list last pushed (the
/// [`crate::ui_unit::UnitFeedState`] resource shape).
///
/// The edge keys live behind a [`crate::ui_script::VmMemo`]: what we last told the VM is a claim
/// about THAT VM (1290), so against a `/reload`'s replacement (1291) — which swaps the VM while
/// every aura stays live — the memo reads fresh and `UNIT_AURA` re-fires for the new frames. That
/// key is also what retired the explicit reset [`end_session_aura_state`] used to run: the memo
/// dies with the VM, while that system keeps its real job, dropping the server-side aura state.
#[derive(Resource, Default)]
struct AuraFeedMemory {
    /// What we last told the VM — dies with the VM it was told to.
    vm: crate::ui_script::VmMemo<AuraFeedMemo>,
    /// The Bevy-clock instant [`trace_timers`] last printed (`BENILLA_AURA_TRACE`). Host
    /// diagnostics, not VM memory — the trace cadence outlives any one VM, deliberately.
    traced_at: f64,
}

/// The per-VM half of [`AuraFeedMemory`] — the `UNIT_AURA` edge keys.
#[derive(Default)]
struct AuraFeedMemo {
    present: bool,
    last: Vec<AuraProjection>,
    /// The tracking spell last pushed ([`tracking_state_of`]) — part of the player half's change
    /// key: switching Find Herbs → Find Minerals changes NO visible list (both are excluded from
    /// the bar), only this, and the minimap frame still needs its event.
    tracking_last: Option<u32>,
    /// The target half: the selection guid + projection last pushed under `"target"` (`None` = the
    /// token is cleared). The guid joins the key so a target *switch* re-fires `UNIT_AURA` even
    /// between two units whose lists happen to project identically.
    target_last: Option<(u64, Vec<AuraProjection>)>,
    /// The pet half, same shape and same reason (decision 0990): a stable swap between two pets
    /// carrying identical auras must still re-fire, so the guid joins the key.
    pet_last: Option<(u64, Vec<AuraProjection>)>,
    /// The target-of-target half (decision 1576) — the ToT frame's four debuff buttons. Same shape
    /// and same reason as the two above: the guid joins the key, because your target swinging from
    /// one identically-debuffed add to another is still a change the row has to hear.
    tot_last: Option<(u64, Vec<AuraProjection>)>,
}

/// The `BENILLA_AURA_TRACE` period in seconds, or `None` when the trace is off. A set-but-unparsable
/// value means "on, at the default 1 Hz" rather than off — a typo shouldn't silently disarm the
/// instrument you asked for (the `WOW_MOVE_TRACE=1` lesson: a knob that quietly means nothing).
fn trace_period() -> Option<f64> {
    static PERIOD: std::sync::OnceLock<Option<f64>> = std::sync::OnceLock::new();
    *PERIOD.get_or_init(|| {
        let v = std::env::var("BENILLA_AURA_TRACE").ok()?;
        Some(
            v.trim()
                .parse::<f64>()
                .ok()
                .filter(|p| *p > 0.0)
                .unwrap_or(1.0),
        )
    })
}

/// One trace tick: for every aura the player's bar is drawing, print the app's remaining time next
/// to the remaining time the *button's own Lua* would render from what it last cached. The bar's
/// countdown is a per-frame poll in the reference (`BuffButton_OnUpdate` → `GetPlayerBuffTimeLeft`,
/// extracted 1.12 `BuffFrame.lua` l.128-130), so these two must agree every frame; a row where they
/// don't is a stale button, which is exactly the "the timer sticks / never appears" symptom.
///
/// The button a given aura lands on is its index *within its own filter* (the bar's ids are
/// 1-based per filter): helpful auras fill `BuffButton0..15`, harmful `BuffButton16..23`.
fn trace_timers(script: &UiScript, cache: &[CachedAura], list: &[AuraState], bevy_now: f64) {
    let script_now = script.now();
    info!(
        "aura trace: {} aura(s), GetTime()={script_now:.2} app clock={bevy_now:.2} (skew {:+.2})",
        list.len(),
        script_now - bevy_now
    );
    let (mut helpful_n, mut harmful_n) = (0usize, 0usize);
    for (c, a) in cache.iter().zip(list) {
        let button = if a.helpful {
            helpful_n += 1;
            helpful_n - 1
        } else {
            harmful_n += 1;
            15 + harmful_n
        };
        let app_left = if a.expiration_time > 0.0 {
            format!("{:.1}s", a.expiration_time - script_now)
        } else {
            "permanent".to_string()
        };
        // What the bar is actually DRAWING — the rendered duration string, not an intermediate.
        // The button holds no expiry of its own (it re-reads one every frame), so the text is the
        // only readback there is, and it is the right one: it is what the director sees.
        let lua = script
            .eval::<String>(&format!(
                r#"local b = getglobal("BuffButton{button}")
                   local d = getglobal("BuffButton{button}Duration")
                   if not b then return "no such button" end
                   return string.format("shown=%s draws %q",
                       tostring(b:IsShown()), d and d:GetText() or "")"#
            ))
            .unwrap_or_else(|e| format!("<lua error: {e}>"));
        info!(
            "  slot {:>2}  spell {:>5}  {:<26}  BuffButton{button:<2}  app left {app_left:<10}  lua {lua}",
            c.slot,
            a.spell_id,
            a.name.as_deref().unwrap_or("<unknown spell>"),
        );
    }
}

fn projection_of(list: &[AuraState]) -> Vec<AuraProjection> {
    list.iter()
        .map(|a| (a.spell_id, a.count, a.cancelable, a.debuff_type.clone()))
        .collect()
}

/// **Any unit but yourself**, read straight off its descriptor — the verified other-unit aura law
/// (decisions 0257/0268): ascending raw slot within the half, durationless, through the same
/// display filter the player bar uses.
///
/// One function for every non-self token (`"target"` since 0255, `"pet"` since 0990) because it is
/// one law: the 1.12 wire carries no duration for another unit (byte-verified, 0257 B6), so the
/// only thing any of them can show is what `UNIT_FIELD_AURA` holds. Filtering here rather than in
/// the Lua binding keeps `UnitBuff(token, i)` returning the i-th *shown* aura, matching the
/// reference's own indices.
///
/// `buffs_visible` is the **unit-level** gate (decision 1035): `UnitBuff 0x519500` decides, once
/// per unit and *before* it walks a single slot, whether this unit's positive half is enumerable at
/// all — see [`buffs_visible_on`]. False drops the helpful half entirely, which is exactly what the
/// reference does (a single nil for every index), so `UnitBuff(token, i)` goes nil while
/// `UnitDebuff(token, i)` is untouched: `UnitDebuff 0x5198f0` has no such gate.
fn other_unit_auras(
    store: &ObjectStore,
    catalog: Option<&SpellCatalog>,
    buffs_visible: bool,
) -> Vec<AuraState> {
    store
        .0
        .unit_auras()
        .filter(|a| buffs_visible || a.slot >= UNIT_AURA_POSITIVE_SLOTS)
        .filter(|a| shown_in_aura_ui(catalog, a.spell_id))
        .map(|a| {
            let display = catalog.and_then(|cat| cat.get(a.spell_id));
            AuraState {
                spell_id: a.spell_id,
                name: display.map(|d| d.name.clone()),
                icon: display.and_then(|d| d.icon.clone()),
                count: a.stacks,
                debuff_type: display
                    .zip(catalog)
                    .and_then(|(d, cat)| cat.dispel_name(d))
                    .map(str::to_string),
                // No duration for any unit but yourself — the 1.12 wire carries none (byte-verified,
                // 0257 B6); the reference's target and pet frames show no timers.
                duration: 0.0,
                expiration_time: 0.0,
                helpful: a.slot < UNIT_AURA_POSITIVE_SLOTS,
                cancelable: a.flags & AURA_FLAG_CANCELABLE != 0,
                // `untilCancelled` is a field of the PLAYER cache record (`0xbc6040`+0xc) and
                // `GetPlayerBuff` is the only reader; no other unit has a cache, and no binding can
                // reach this flag through a non-player token. A self-target does not come through
                // here at all — it mirrors the player list (decision 0257 §2).
                until_cancelled: false,
                channeled: display
                    .is_some_and(|d| d.attributes_ex & SPELL_ATTR_EX_IS_CHANNELED != 0),
            }
        })
        .collect()
}

/// **`UnitBuff 0x519500`'s unit-level gate** — may this unit's *buffs* be enumerated at all?
/// (Decision 1035.)
///
/// This sits **before** the per-slot walk and outranks it: when it fails, `UnitBuff` pushes a
/// single nil for **every** index without examining one slot, so the unit shows *no buffs*
/// whatever its aura array holds. `UnitDebuff 0x5198f0` has no counterpart — debuffs are always
/// enumerable, which is why a hostile mob shows its debuffs and nothing else.
///
/// ```text
/// (unit.UNIT_FIELD_FLAGS & UNIT_FLAG_AURAS_VISIBLE)        // 0x08000000
///   || (CanAssist(activePlayer, unit) && activePlayer.UNIT_FIELD_CHARMEDBY == 0)
/// ```
///
/// The first clause is **Detect Magic** (spell 2855 → `SPELL_AURA_AURAS_VISIBLE`, vmangos
/// `UnitDefines.h:518` *"Detect Magic. Added by SPELL_AURA_AURAS_VISIBLE"*). That spell exists
/// precisely *because* the second clause denies a hostile unit's buffs — which is the independent
/// gameplay corroboration that this gate is real and not an artifact of one screenshot.
///
/// **It is also why GM mode changes the answer**, and why the same mob read two ways for a while:
/// vmangos ORs the bit into the outgoing `UNIT_FIELD_FLAGS` *for GameMasters only* (`Object.cpp`
/// 765-766, *"Gamemasters should be always able to select units and view auras"*). So the server
/// already tailors this per viewer and we need know nothing about GM state — reading the flag off
/// the descriptor as received is both simpler and exactly right in both regimes.
fn buffs_visible_on(
    store: &ObjectStore,
    self_store: Option<&ObjectStore>,
    factions: Option<&Factions>,
    reputations: &Reputations,
    owner_store: impl FnOnce(u64) -> Option<ObjectStore>,
) -> bool {
    if store.0.unit_flags() & UNIT_FLAG_AURAS_VISIBLE != 0 {
        return true;
    }
    // `activePlayer.UNIT_FIELD_CHARMEDBY == 0` — while something else is driving you, you assist
    // nobody (`0x5195f1`-`0x5195f7`).
    if self_store.is_some_and(|s| s.0.unit_charmed_by().is_some()) {
        return false;
    }
    can_assist(Some(store), factions, reputations, self_store, owner_store)
}

/// `UNIT_FLAG_AURAS_VISIBLE` — see [`buffs_visible_on`].
const UNIT_FLAG_AURAS_VISIBLE: u32 = 0x0800_0000;

/// `Spell.dbc` `AttributesEx & 0x4` — `SPELL_ATTR_EX_IS_CHANNELED` (vmangos `SpellDefines.h`), the
/// DBC arm of the cancel gate (`benilla_ui::script::aura`'s `cancel_authorized`, `0x4e4a10`).
const SPELL_ATTR_EX_IS_CHANNELED: u32 = 0x4;

/// The cache record's **`untilCancelled`** flag — `GetPlayerBuff`'s second return, `+0xc` of a
/// `0xbc6040` record. "This aura has no finite duration to display."
///
/// Byte-verified against `BuildBuffRecord 0x4e44b0`'s tail (`0x4e452e`-`0x4e45c5`), which is two
/// clauses ORed in sequence:
///
/// 1. **The DBC duration row.** `SpellRec+0x78` (`DurationIndex`) resolves a `SpellDuration.dbc` row;
///    the flag is set when the row says *infinite* — `Duration < 0 && DurationPerLevel <= 0`
///    (`0x4e456e`-`0x4e457a`) — or when the index resolves to no row at all (`jl`/`jg`/NULL all fall
///    into the same `mov ecx,1` at `0x4e4580`).
/// 2. **The area-aura override.** Only for a *non*-cancelable aura (`test [edi+0xa],0x1; jne` at
///    `0x4e4588` returns early otherwise): if any of the spell's three effects is one of
///    `{0x23, 0x77, 0x80, 0x81}` — the persistent/area-aura effects, which have no per-application
///    timer because they last while you stand in them — the flag is **forced** to 1
///    (`0x4e458e`-`0x4e45be`).
///
/// This closes the gap this module's header named ("the reference avoids this via a DBC 'until
/// cancelled' flag we don't parse"). It matters because it is answerable from the *spell*, on the
/// frame the aura appears: `ref-BuffFrame.lua:124` returns before ever calling
/// `GetPlayerBuffTimeLeft` when it reads 1, so a permanent aura never flashes a `0 s` countdown
/// while waiting for a duration packet that is never coming.
///
/// **The one inference.** When the catalog cannot resolve the spell at all, the reference leaves
/// `+0xc` holding whatever the record's previous occupant left there (its early-outs at `0x4e4533`/
/// `0x4e4550` skip the write) — stale memory, not a mechanism. We answer from the only evidence we
/// still have: no expiry stamp joined ⇒ nothing finite to display. A catalog miss already costs the
/// aura its icon and name, so this only decides whether a nameless icon also shows `0 s`.
fn until_cancelled(
    display: Option<&benilla_formats::SpellDisplay>,
    duration_row: Option<&benilla_formats::SpellDuration>,
    cancelable: bool,
    expiration_time: f64,
) -> bool {
    let Some(d) = display else {
        return expiration_time == 0.0;
    };
    let permanent = match duration_row {
        Some(row) => row.base_ms < 0 && row.per_level_ms <= 0,
        // No row — a `DurationIndex` that is negative, out of range, or resolves to nothing. The
        // reference funnels all three into the same `mov ecx,1` at `0x4e4580`.
        None => true,
    };
    permanent
        || (!cancelable
            && d.effects
                .iter()
                .any(|e| matches!(e, 0x23 | 0x77 | 0x80 | 0x81)))
}

/// The reference's aura display filter (decisions 0268 + 0417): an aura is shown iff its spell is
/// *not* flagged never-display (`SPELL_ATTR_DO_NOT_DISPLAY` / `SPELL_ATTR_EX_NO_AURA_ICON`, via
/// `SpellDisplay::hidden_from_aura_bar`) **and** is not a tracking spell
/// (`SpellDisplay::tracking_aura` — the `{0x2c,0x2d,0x97}` effect exclusion both filters carry:
/// the player-cache rebuild `PlayerAuras_Update 0x4e4170` skips a tracking aura *before* the
/// insert, diverting it to the tracking global instead, and `IsAuraDisplayable 0x519860` hides it
/// from other units' rows the same way). This holds on **every** aura display — the player's own
/// bar and any other unit's rows alike (the director watched the reference hide a target's Battle
/// Stance; 0417 corrects 0268's player-only scope note). A spell the catalog can't resolve stays
/// visible — fail-open, like every other catalog miss in the feed.
///
/// **That fail-open is ours, not the reference's, on this path** — the parenthetical that used to
/// justify it here ("like the reference's own no-SpellRec path, which inserts") was true of the
/// *player cache* and is FALSE of the non-player walk: `UnitBuff`/`UnitDebuff` **skip** a slot
/// whose id has no `Spell.dbc` row (`id > [0xc0d78c]` or a null row; decision 1035).
/// Kept as fail-open deliberately: every id on the wire is a real spell, so the two behaviours are
/// indistinguishable in practice, and failing *closed* would let a catalog load hiccup silently
/// blank every aura on every frame. A knowing divergence, recorded — not a comment asserting
/// something untrue.
fn shown_in_aura_ui(catalog: Option<&SpellCatalog>, spell_id: u32) -> bool {
    catalog
        .and_then(|c| c.get(spell_id))
        .is_none_or(|d| !d.hidden_from_aura_bar() && !d.tracking_aura())
}

/// The player's active tracking aura — the reference's tracking global (`DAT_00bc6378`): the
/// cache rebuild walks the raw slots **ascending** and each
/// visible tracking-effect aura overwrites it, so the LAST one wins. The attribute clauses are
/// tested *first* in the reference (the `goto` skips the effect loop), so an attribute-hidden
/// tracking spell never lands here; a catalog miss can't be identified as tracking and lands in
/// the bar instead (the reference's own no-SpellRec path). Read by `GetTrackingTexture` /
/// `CancelTrackingBuff` / `GameTooltip:SetTrackingSpell` via [`UiScript::set_tracking`].
fn tracking_state_of(
    catalog: Option<&SpellCatalog>,
    occupied: &[UnitAuraSlot],
) -> Option<TrackingState> {
    occupied.iter().rev().find_map(|a| {
        let d = catalog.and_then(|c| c.get(a.spell_id))?;
        (!d.hidden_from_aura_bar() && d.tracking_aura()).then(|| TrackingState {
            spell_id: a.spell_id,
            name: Some(d.name.clone()),
            icon: d.icon.clone(),
            cancelable: a.flags & AURA_FLAG_CANCELABLE != 0,
        })
    })
}

fn feed_auras(
    script: Option<NonSendMut<UiScript>>,
    self_q: Query<(&ObjectStore, &Guid), With<SelfPlayer>>,
    selection: Res<Selection>,
    stores: Query<&ObjectStore>,
    spells: Option<Res<Spells>>,
    // The pet half (decision 0990): the bar owns the pet's identity, the index resolves it.
    pet: Res<crate::ui_pet::PetBar>,
    index: Res<GuidIndex>,
    // The `UnitBuff` unit-level gate's inputs ([`buffs_visible_on`]) — the same reaction resolve
    // the selection ring and the name plates run, so one law answers "is this unit friendly to me"
    // everywhere.
    factions: Option<Res<Factions>>,
    reputations: Res<Reputations>,
    // Read-only over the stamps: the feed joins them, the net apply path writes them, and only the
    // session end drops them (decision 0900). Nothing the feed sees may invalidate one.
    durations: Res<AuraDurations>,
    mut cache: ResMut<PlayerAuraCache>,
    time: Res<Time<Real>>,
    mut mem: ResMut<AuraFeedMemory>,
) {
    let Some(mut script) = script else {
        return;
    };
    let Ok((store, self_guid)) = self_q.single() else {
        // No avatar entity — and that is NOT, on its own, the end of the aura state, so nothing is
        // torn down here (that is [`end_session_aura_state`]'s job, on the session edge). A
        // cross-map worldport despawns every tracked entity, ours included, and the new map
        // re-streams it under a fresh entity with the same guid (`net::apply::tag_self_player`'s
        // own note); for the frames of that gap there is no avatar while every aura on us is still
        // live, and the server sends nothing to rebuild the timers with — vmangos emits
        // `SMSG_UPDATE_AURA_DURATION` only on an aura's apply/refresh and at
        // `Map::ExistingPlayerLogin` (the already-online relog), never on a worldport.
        //
        // The reference has nothing to lose across that gap: its buff cache (`0xbc6040`) and expiry
        // array (`0xbc5f68`) are process globals, zeroed once at startup (`0x4e40c0`, reached only
        // from the one-shot init at `0x48f5a9`, itself called only from `0x401602`) and otherwise
        // touched by exactly two sites — the duration packet's setter `0x4e43a6` and
        // `GetPlayerBuffTimeLeft`'s reader `0x4e4467`. That is the whole xref set: `0xbc5f68`
        // appears three times in `WoW.exe`. Nothing clears them on a world change, so a debuff's
        // countdown simply carries across the loading screen. Clearing here was the "after a tele
        // my disease has no timer" defect (decision 0900): the stamps were dropped, and even
        // without that every survivor's `appeared_at` restarted at the re-stream, so the freshness
        // gate below would have rejected whatever stamps remained.
        return;
    };

    let bevy_now = time.elapsed_secs_f64();
    let catalog = spells.as_ref().map(|s| &s.catalog);
    // `SpellDuration.dbc`, for the cache record's `untilCancelled` flag ([`until_cancelled`]) — the
    // same catalog the `$d` tooltip token resolves against, so nothing new is loaded for this.
    let spell_durations = spells.as_ref().map(|s| &s.durations);
    let occupied: Vec<UnitAuraSlot> = store.0.unit_auras().collect();
    // The reference's DISPLAY FILTER (byte-verified sites `0x4e42b6`–`0x4e42c8`; decisions
    // 0268 + 0385, and 0417 for the target frame): a slot whose spell carries
    // `SPELL_ATTR_DO_NOT_DISPLAY` / `SPELL_ATTR_EX_NO_AURA_ICON` (warrior stances) stays live in
    // `UNIT_FIELD_AURA` but is never shown — `NO_AURA_ICON` means exactly that, on every aura display.
    // For the player's bar we filter the cache's *input*, so a hidden aura never takes a cache
    // position and insertion order/repacking match the reference's; [`shown_in_aura_ui`] is the same
    // predicate the target rows apply below. A catalog miss stays visible — fail-open.
    let live: Vec<UnitAuraSlot> = occupied
        .iter()
        .filter(|a| shown_in_aura_ui(catalog, a.spell_id))
        .copied()
        .collect();
    reconcile(&mut cache.auras, &live, bevy_now);

    // NOTE: there is deliberately no "prune the stamps of empty slots" pass here. It was the whole
    // of the "a freshly cast buff shows no timer, ever" defect: the duration packet leads its own
    // descriptor delta (decision 0257 B6 — measured at one frame, ~50 ms, on a fresh apply), so at
    // the instant a stamp arrives its slot is still EMPTY, and an occupancy prune deletes it
    // sub-millisecond later, before the aura it belongs to can appear. Staleness belongs to the
    // freshness gate below, which compares a stamp against the aura it would be joined to rather
    // than against a slot's momentary emptiness — the defence 0257 §3 built for exactly this.
    // The map cannot grow: it is keyed by raw slot (≤ 48), each entry overwritten by the next
    // packet for that slot, and cleared with the rest of the aura state when the session ends.

    let script_now = script.now();

    let list: Vec<AuraState> = cache
        .auras
        .iter()
        .map(|c| {
            let display = catalog.and_then(|cat| cat.get(c.spell_id));
            let (duration, expiration_time) = join_duration(
                durations.by_slot.get(&c.slot),
                c.appeared_at,
                bevy_now,
                script_now,
            );
            AuraState {
                spell_id: c.spell_id,
                name: display.map(|d| d.name.clone()),
                icon: display.and_then(|d| d.icon.clone()),
                count: c.stacks,
                debuff_type: display
                    .zip(catalog)
                    .and_then(|(d, cat)| cat.dispel_name(d))
                    .map(str::to_string),
                duration,
                expiration_time,
                helpful: c.slot < UNIT_AURA_POSITIVE_SLOTS,
                cancelable: c.flags & AURA_FLAG_CANCELABLE != 0,
                until_cancelled: until_cancelled(
                    display,
                    display.and_then(|d| spell_durations.and_then(|c| c.get(d.duration_index))),
                    c.flags & AURA_FLAG_CANCELABLE != 0,
                    expiration_time,
                ),
                channeled: display
                    .is_some_and(|d| d.attributes_ex & SPELL_ATTR_EX_IS_CHANNELED != 0),
            }
        })
        .collect();

    // The tracking global's benilla twin: derived from the same walk that excluded tracking spells
    // from `live` above, before the memory update so its change joins the edge key.
    let tracking = tracking_state_of(catalog, &occupied);
    let tracking_spell = tracking.as_ref().map(|t| t.spell_id);

    // The TIMER trace (`BENILLA_AURA_TRACE=<secs>`, default 1 s): the instrument for "the countdown
    // is wrong" reports. Every tick it prints, per live aura, the app's own remaining time beside
    // what the *bar's Lua* actually holds — the two halves that can silently disagree, since the
    // app recomputes the expiry every frame while a button only re-reads it when something makes it
    // repaint. A row whose `lua left` has drifted from `app left` is the disagreement, named.
    if let Some(period) = trace_period() {
        if bevy_now - mem.traced_at >= period {
            mem.traced_at = bevy_now;
            trace_timers(&script, &cache.auras, &list, bevy_now);
        }
    }

    // The VM half — resolved against THIS VM, so a `/reload`'s replacement (1291) reads a fresh
    // memo and every edge below re-fires for it. Taken after the `traced_at` use above: the trace
    // cadence is host diagnostics and must not expire with the VM.
    let memo = mem.vm.get(&script);

    // Edge-trigger UNIT_AURA on a discrete change (the reference's PLAYER_AURAS_CHANGED), never on
    // the countdown alone — that's a per-frame OnUpdate on the button, not an event. The tracking
    // spell joins the key: a tracking *switch* changes no visible list, only the minimap icon.
    let projection = projection_of(&list);
    let changed = !memo.present || projection != memo.last || tracking_spell != memo.tracking_last;
    memo.last = projection;
    memo.tracking_last = tracking_spell;
    memo.present = true;

    // Debug affordance (`BENILLA_AURA_DUMP=1`): when the visible set changes, log every slot the
    // player's bar will draw — raw slot, spell id, resolved name, class, and the flags nibble. The
    // fastest answer to "what is actually on my bar, and should any of it be there?" — the husk that
    // `unit_aura`'s `& 0x0E` gate now hides never reaches here, so anything listed is a live aura
    // (decisions 0255/0257). Cheap: the env is only consulted on a change.
    if changed && std::env::var_os("BENILLA_AURA_DUMP").is_some() {
        info!("aura dump: {} aura(s) on the player bar", cache.auras.len());
        for c in &cache.auras {
            let name = catalog
                .and_then(|cat| cat.get(c.spell_id))
                .map(|d| d.name.as_str())
                .unwrap_or("<unknown spell>");
            info!(
                "  slot {:>2}  spell {:>5}  {:<26}  {:<6}  flags {:#06b}",
                c.slot,
                c.spell_id,
                name,
                if c.slot < UNIT_AURA_POSITIVE_SLOTS {
                    "buff"
                } else {
                    "debuff"
                },
                c.flags,
            );
        }
    }

    // The target's list (the target frame's aura rows — 0255's deferred slice). A self-target
    // mirrors the player list (decision 0257 §2: the player-bar law under every token); any other
    // unit is its descriptor read straight — ascending raw slot, durationless (the verified
    // other-unit law, 0257/0268) — but through the SAME display filter (`shown_in_aura_ui`): a
    // never-display aura (a warrior stance) is hidden here too, exactly as the reference hides it
    // (decision 0417 — the director's Battle-Stance-on-the-target report; corrects 0268's player-only
    // scope note). Filtering here rather than in the Lua binding keeps `UnitBuff("target", i)`
    // returning the i-th *shown* aura, matching the reference's own indices.
    let target_list: Option<Vec<AuraState>> =
        selection.target.zip(selection.guid).and_then(|(e, guid)| {
            if guid == self_guid.0 {
                return Some(list.clone());
            }
            // NOT named `store` — that is the SELF player's, bound above and needed here as the
            // gate's second argument.
            let target_store = stores.get(e).ok()?;
            // The gate is resolved per unit, once, exactly where the reference resolves it —
            // before any slot is read (decision 1035).
            let buffs = buffs_visible_on(
                target_store,
                Some(store),
                factions.as_deref(),
                &reputations,
                |owner| {
                    index
                        .0
                        .get(&owner)
                        .and_then(|&e| stores.get(e).ok())
                        .cloned()
                },
            );
            Some(other_unit_auras(target_store, catalog, buffs))
        });

    // The pet's list — the pet frame's four debuff buttons (decision 0990). Same other-unit law as
    // the target above, and deliberately the same function: a pet is not a special case of it.
    // The token is the bar's cached pet guid ([`crate::ui_pet::feed_pet_unit`]'s own note on why
    // that word and not `UNIT_FIELD_SUMMON`). A self-mirror leg would be dead code here — no unit
    // is ever its own pet.
    let pet_guid = pet.spells.pet_guid;
    let pet_list: Option<Vec<AuraState>> = (pet_guid != 0)
        .then(|| index.0.get(&pet_guid))
        .flatten()
        .and_then(|&e| stores.get(e).ok())
        // **The pet keeps the ungated read, deliberately** (decision 1035). A pet is
        // PLAYER_CONTROLLED, which sends `CanAssist` down an arm (`0x60673e`-`0x60679f`, keyed on
        // an `[obj+0xe68]` record) not yet named — so the gate is not known here, and
        // guessing it is the one mistake that could blank a working pet frame. `true` is the
        // status quo; the pet frame draws only debuffs today anyway (0990), so nothing observable
        // rides on it until that arm is derived.
        .map(|store| other_unit_auras(store, catalog, true));

    // The target's target's list — the ToT frame's four debuff buttons (decision 1576). Same
    // other-unit law as the target and the pet above, resolved through the same one-hop
    // `UNIT_FIELD_TARGET` read the unit feed's `"targettarget"` snapshot uses. The self-mirror leg
    // is NOT dead code here the way it would be for a pet: while you tank, your target's target is
    // YOU, and 0257 §2's player-bar law is what the row must draw then.
    let tot_guid = selection
        .target
        .and_then(|e| stores.get(e).ok())
        .and_then(|s| s.0.unit_target())
        .filter(|g| *g != 0)
        .unwrap_or(0);
    let tot_list: Option<Vec<AuraState>> = (tot_guid != 0)
        .then(|| index.0.get(&tot_guid))
        .flatten()
        .and_then(|&e| {
            if tot_guid == self_guid.0 {
                return Some(list.clone());
            }
            let tot_store = stores.get(e).ok()?;
            let buffs = buffs_visible_on(
                tot_store,
                Some(store),
                factions.as_deref(),
                &reputations,
                |owner| {
                    index
                        .0
                        .get(&owner)
                        .and_then(|&e| stores.get(e).ok())
                        .cloned()
                },
            );
            Some(other_unit_auras(tot_store, catalog, buffs))
        });

    let target_cur = selection
        .guid
        .zip(target_list.as_deref())
        .map(|(guid, l)| (guid, projection_of(l)));
    let target_changed = target_cur.is_some() && target_cur != memo.target_last;
    memo.target_last = target_cur;

    // The BENILLA_AURA_DUMP affordance's target half: what the target rows will draw, on change.
    if target_changed && std::env::var_os("BENILLA_AURA_DUMP").is_some() {
        let l = target_list.as_deref().unwrap_or_default();
        info!("aura dump: {} aura(s) on the target rows", l.len());
        for a in l {
            info!(
                "  spell {:>5}  {:<26}  {:<6}  {}",
                a.spell_id,
                a.name.as_deref().unwrap_or("<unknown spell>"),
                if a.helpful { "buff" } else { "debuff" },
                a.debuff_type.as_deref().unwrap_or("-"),
            );
        }
    }

    let pet_cur = (pet_guid != 0)
        .then_some(pet_list.as_deref())
        .flatten()
        .map(|l| (pet_guid, projection_of(l)));
    let pet_changed = pet_cur.is_some() && pet_cur != memo.pet_last;
    memo.pet_last = pet_cur;

    let tot_cur = (tot_guid != 0)
        .then_some(tot_list.as_deref())
        .flatten()
        .map(|l| (tot_guid, projection_of(l)));
    let tot_changed = tot_cur.is_some() && tot_cur != memo.tot_last;
    memo.tot_last = tot_cur;

    script.set_auras("player", Some(list));
    // Clearing the token isn't a UNIT_* event (the frame reacts to PLAYER_TARGET_CHANGED, and the
    // pet frame to UNIT_PET) — same convention as the unit feed's snapshot clear.
    script.set_auras("target", target_list);
    script.set_auras("pet", pet_list);
    script.set_auras("targettarget", tot_list);
    script.set_tracking(tracking);
    if changed {
        script.fire_event("UNIT_AURA", vec![ScriptValue::Str("player".into())]);
        // The reference's own event for the same rebuild — PLAYER_AURAS_CHANGED, no args — which
        // the verbatim-transcribed 1.12 frames register: MiniMapTrackingFrame, the buff bar since
        // it came onto the `GetPlayerBuff` family, and every corpus aura addon. Fired beside the
        // Era-shaped UNIT_AURA the unit frames listen on: one rebuild, both dialects.
        script.fire_event("PLAYER_AURAS_CHANGED", vec![]);
    }
    if pet_changed {
        script.fire_event("UNIT_AURA", vec![ScriptValue::Str("pet".into())]);
    }
    if target_changed {
        script.fire_event("UNIT_AURA", vec![ScriptValue::Str("target".into())]);
    }
    if tot_changed {
        script.fire_event("UNIT_AURA", vec![ScriptValue::Str("targettarget".into())]);
    }
}

/// The apply/refresh-to-descriptor slack: a duration packet is accepted for an aura if it arrived no
/// more than this long before the aura appeared. Generous versus the **measured** lead — one client
/// frame, ~50 ms, on a fresh apply against the live server (decision 0846) — and tight versus the
/// seconds a stale recycled-slot stamp would be off by.
const DURATION_SLACK: f64 = 1.0;

/// Join a slot's duration stamp to the aura now sitting in that slot, yielding `UnitBuff`'s
/// `(duration, expirationTime)` pair on the **script** clock (`0.0, 0.0` = no timer, our "until
/// cancelled"). This is the *only* thing that may reject a stamp, and it is the right place for it:
/// the stamp is compared against the aura it would be joined to, never against the slot's momentary
/// occupancy. `SMSG_UPDATE_AURA_DURATION` leads its own descriptor delta (decision 0257 B6), so at
/// the instant a stamp arrives its slot is *empty* — a rule phrased on occupancy deletes exactly the
/// packet that matters, which is decision 0846's first defect.
///
/// A stamp that predates the aura by more than [`DURATION_SLACK`] belonged to a since-expired
/// occupant of a recycled slot, and is dropped (decision 0257 §3 — our stand-in for the reference's
/// unparsed `SpellDuration.dbc` "until cancelled" flag).
fn join_duration(
    stamp: Option<&DurationStamp>,
    appeared_at: f64,
    bevy_now: f64,
    script_now: f64,
) -> (f64, f64) {
    stamp
        .filter(|d| d.received_at >= appeared_at - DURATION_SLACK)
        .map(|d| (d.total, script_now + (d.expires_at - bevy_now)))
        .unwrap_or((0.0, 0.0))
}

/// The end of the session (`OnExit(ClientState::InWorld)` — a confirmed `/logout`, back to the glue
/// layer): drop the whole aura state, lists and duration stamps alike, so nothing reaches the next
/// character's bar.
///
/// This is the *only* place the state is dropped, and the edge it hangs off is the point. The
/// reference drops nothing, ever — `0xbc5f68` is zeroed once at client startup and written only by
/// the duration packet (decision 0900) — because at this boundary it was protected by a server that
/// re-sent every aura's duration in its login preamble; mangos still carries the bare
/// `// SMSG_UPDATE_AURA_DURATION` placeholder where that packet went in
/// `Player::SendInitialPacketsBeforeAddToMap`, and vmangos never fills it in. So a stamp we kept
/// across a logout would sit in its raw slot with nothing to overwrite it, and the next character's
/// aura in that slot could inherit a timer that was never theirs. One divergence from the reference,
/// at the one boundary where the reference relied on a server we don't have.
fn end_session_aura_state(
    script: Option<NonSendMut<UiScript>>,
    mut durations: ResMut<AuraDurations>,
    mut cache: ResMut<PlayerAuraCache>,
) {
    if let Some(mut script) = script {
        script.set_auras("player", None);
        script.set_auras("target", None);
        script.set_auras("targettarget", None);
        script.set_tracking(None);
    }
    cache.auras.clear();
    durations.by_slot.clear();
    // No [`AuraFeedMemory`] reset here: its edge keys live behind a `VmMemo` and die with the VM
    // this session's pushes went to (1290/1291) — only the server-side state above needs a hand.
}

/// Drain the spell ids `CancelUnitBuff` queued this frame and send one `CMSG_CANCEL_AURA` each. The
/// server cancels by spell, not slot (decision 0257 B8); it refuses anything the wire's
/// `AFLAG_CANCELABLE` bit didn't allow, which the binding already gated on.
fn drain_aura_cancels(script: Option<NonSendMut<UiScript>>, net: Res<NetCommands>) {
    let Some(mut script) = script else {
        return;
    };
    for spell_id in script.take_cancel_aura_requests() {
        let _ = net.0.send(ClientCommand::CancelAura { spell_id });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The plugin's own wiring, on a bare app: enough resources for the Update systems to run, and
    /// the state machine the teardown hangs off. No `UiScript` (it is `NonSend` and needs the whole
    /// FrameXML VM) — the systems that touch it take it as an `Option` and skip, which is exactly
    /// what this test wants to observe: what happens to the *state* across the session edge.
    fn aura_app() -> App {
        // The cancel drain's outbound channel. Nothing sends on it here (its only source is the
        // script VM, which this app has none of), so the dropped receiver is inert.
        let (tx, _) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .insert_state(ClientState::InWorld)
            .init_resource::<Selection>()
            // The pet half's two inputs (decision 0990). Left empty here — this app has no pet, so
            // the `"pet"` token stays cleared and the feed's pet leg is inert.
            .init_resource::<crate::ui_pet::PetBar>()
            .init_resource::<GuidIndex>()
            // The `UnitBuff` gate's reaction inputs (decision 1035). `Factions` is an
            // `Option<Res<..>>` (it needs the DBC catalog), but `Reputations` is not — the feed
            // reads it unconditionally, so a bare app must carry it or the system fails validation.
            .init_resource::<Reputations>()
            .insert_resource(NetCommands(tx))
            .add_plugins(UiAuraPlugin);
        app
    }

    /// Seed the app with one live aura carrying a running timer — the state a teleport must not
    /// disturb and a logout must.
    fn seed_one_timed_aura(app: &mut App) {
        app.world_mut()
            .resource_mut::<AuraDurations>()
            .set(32, 300_000, 10.0);
        app.world_mut()
            .resource_mut::<PlayerAuraCache>()
            .auras
            .push(CachedAura {
                slot: 32, // the first harmful slot — a debuff, like the report's disease
                spell_id: 11976,
                appeared_at: 10.0,
                flags: 0x8,
                level: 60,
                stacks: 1,
            });
    }

    /// **Decision 0900, pinned.** A cross-map worldport despawns every tracked entity — our avatar
    /// included — and the new map re-streams it, so for a stretch of frames mid-session there is no
    /// `SelfPlayer` while every aura on us is still live. Nothing may tear the aura state down on
    /// that: vmangos re-sends `SMSG_UPDATE_AURA_DURATION` only on apply/refresh (and at an
    /// already-online relog), never on a worldport, so a dropped stamp is gone for the aura's whole
    /// remaining life — the director's "after a tele my disease has no timer".
    ///
    /// The session edge is the one place it *does* go, and this pins both halves — the second one
    /// squarely, the first one for what a VM-less app can reach: the Update schedule leaves a
    /// seeded, avatar-less aura state alone. The teardown having exactly one site, on that edge, is
    /// what makes the first half hold in the real app, and that is structural, not asserted here.
    #[test]
    fn the_aura_state_survives_an_avatar_less_frame_and_dies_only_with_the_session() {
        let mut app = aura_app();
        seed_one_timed_aura(&mut app);

        // Frames with no avatar entity at all — the worldport gap.
        app.update();
        app.update();
        assert_eq!(
            app.world().resource::<PlayerAuraCache>().auras.len(),
            1,
            "the display cache survives the gap — its `appeared_at` is the freshness gate's anchor"
        );
        assert!(
            app.world()
                .resource::<AuraDurations>()
                .by_slot
                .contains_key(&32),
            "the duration stamp survives the gap — nothing will re-send it"
        );

        // The session ends: `/logout` back to the glue layer.
        app.world_mut()
            .resource_mut::<NextState<ClientState>>()
            .set(ClientState::CharSelect);
        app.update();
        assert!(app.world().resource::<PlayerAuraCache>().auras.is_empty());
        assert!(app.world().resource::<AuraDurations>().by_slot.is_empty());
        // The feed's edge memory needs no teardown assertion any more: it sits behind a `VmMemo`,
        // so it dies with the VM rather than on this edge — the property `ui_script::session`'s
        // own tests pin (1290/1291), and one a VM-less app cannot observe.
    }

    fn slot(slot: u8, spell_id: u32) -> UnitAuraSlot {
        UnitAuraSlot {
            slot,
            spell_id,
            flags: if slot < UNIT_AURA_POSITIVE_SLOTS {
                AURA_FLAG_CANCELABLE | 0x8
            } else {
                0x8
            },
            level: 60,
            stacks: 1,
        }
    }

    fn order(cache: &[CachedAura]) -> Vec<(u8, u32)> {
        cache.iter().map(|c| (c.slot, c.spell_id)).collect()
    }

    /// The display filter both the player bar and the target rows run through (`shown_in_aura_ui`),
    /// exercised against the REAL 5875 `Spell.dbc`: a warrior's Battle Stance carries `NO_AURA_ICON`
    /// and is hidden on every frame (decision 0417 — the director's "battle stance on the target
    /// frame" report), while Battle Shout is a real buff that stays. Skips without client data.
    #[test]
    fn the_aura_display_filter_hides_a_real_battle_stance_but_keeps_battle_shout() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let catalog = benilla_formats::load_spell_catalog(&mut chain).expect("Spell.dbc");

        // A target carrying Battle Stance (2457) + Battle Shout (6673): the stance is filtered out of
        // what the rows draw, exactly as the reference hides it; the shout survives.
        let slots = [slot(0, 2457), slot(1, 6673)];
        let shown: Vec<u32> = slots
            .iter()
            .filter(|a| shown_in_aura_ui(Some(&catalog), a.spell_id))
            .map(|a| a.spell_id)
            .collect();
        assert_eq!(shown, [6673], "the stance is filtered, the shout stays");

        // Fail-open: no catalog at all, or an id the catalog can't resolve, stays visible.
        assert!(shown_in_aura_ui(None, 2457));
        assert!(shown_in_aura_ui(Some(&catalog), 0xffff_fffe));
    }

    /// The tracking half of the display filter (Pass 2 of `PlayerAuras_Update 0x4e4170`: the
    /// `{0x2c,0x2d,0x97}` effect exclusion + the tracking global), against the REAL 5875
    /// `Spell.dbc`: a tracking aura rides a visible `UNIT_FIELD_AURA` slot but never reaches any
    /// bar — it is diverted to the tracking state instead, and the ascending walk's LAST tracking
    /// aura wins the global. Skips without client data.
    #[test]
    fn a_real_tracking_aura_is_diverted_from_the_bar_to_the_tracking_state() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let catalog = benilla_formats::load_spell_catalog(&mut chain).expect("Spell.dbc");

        // Find Herbs 2383 (TRACK_RESOURCES 45), Battle Shout 6673 (an ordinary buff), Track
        // Beasts 1494 (TRACK_CREATURES 44): only the shout is shown on any aura display...
        let slots = [slot(0, 2383), slot(1, 6673), slot(2, 1494)];
        let shown: Vec<u32> = slots
            .iter()
            .filter(|a| shown_in_aura_ui(Some(&catalog), a.spell_id))
            .map(|a| a.spell_id)
            .collect();
        assert_eq!(
            shown,
            [6673],
            "both tracking auras are diverted, the shout stays"
        );

        // ...and the tracking global holds the LAST tracking aura of the ascending walk (each
        // match overwrites), with the display fields the icon + tooltip read.
        let t = tracking_state_of(Some(&catalog), &slots).expect("tracking state set");
        assert_eq!(
            t.spell_id, 1494,
            "slot 2's Track Beasts overwrites slot 0's Find Herbs"
        );
        assert_eq!(t.name.as_deref(), Some("Track Beasts"));
        assert!(t.icon.is_some(), "the icon path GetTrackingTexture returns");
        assert!(
            t.cancelable,
            "the synthesized slot carries AFLAG_CANCELABLE"
        );

        // No tracking aura live → no state (the frame's hide branch); and without a catalog no
        // aura can be identified as tracking (the reference's own no-SpellRec path inserts into
        // the bar instead — fail-open both sides of the diversion).
        assert!(tracking_state_of(Some(&catalog), &[slot(0, 6673)]).is_none());
        assert!(tracking_state_of(None, &slots).is_none());
    }

    /// The distinguishing behaviour of decision 0257: a newly-applied aura in a *lower* slot
    /// appends at the END of the cache, it is not sorted to the front by slot. A plain
    /// `unit_auras()` read (ascending slot) would give the opposite order — this is exactly the
    /// difference between the descriptor's order and the display order.
    #[test]
    fn a_new_low_slot_aura_appends_at_the_end_not_sorted_by_slot() {
        let mut cache = Vec::new();
        // X lands in slot 5 first.
        reconcile(&mut cache, &[slot(5, 100)], 1.0);
        assert_eq!(order(&cache), [(5, 100)]);
        // Y then lands in slot 2 — the descriptor now reads ascending [2, 5], but Y is the newer
        // aura, so it goes to the end.
        reconcile(&mut cache, &[slot(2, 200), slot(5, 100)], 2.0);
        assert_eq!(
            order(&cache),
            [(5, 100), (2, 200)],
            "insertion order, not ascending slot"
        );
    }

    /// A dropped aura closes its gap; the survivors keep their relative order and slide along — the
    /// `PlayerAuras_Update` shift-down. A recycled slot then appends fresh, not into the hole.
    #[test]
    fn a_dropped_aura_repacks_and_a_recycled_slot_appends_fresh() {
        let mut cache = Vec::new();
        reconcile(&mut cache, &[slot(0, 10), slot(1, 20), slot(2, 30)], 1.0);
        assert_eq!(order(&cache), [(0, 10), (1, 20), (2, 30)]);

        // The middle aura (slot 1) drops.
        reconcile(&mut cache, &[slot(0, 10), slot(2, 30)], 2.0);
        assert_eq!(order(&cache), [(0, 10), (2, 30)], "the gap closes");

        // Slot 1 is recycled by a new spell — it appends at the end, not back into the old middle.
        reconcile(&mut cache, &[slot(0, 10), slot(1, 99), slot(2, 30)], 3.0);
        assert_eq!(
            order(&cache),
            [(0, 10), (2, 30), (1, 99)],
            "the recycled slot is the newest, so it is last"
        );
    }

    /// A surviving aura refreshes its volatile fields in place (a stack change) without moving.
    #[test]
    fn a_surviving_aura_refreshes_in_place() {
        let mut cache = Vec::new();
        reconcile(&mut cache, &[slot(0, 10), slot(1, 20)], 1.0);
        let mut restacked = slot(1, 20);
        restacked.stacks = 5;
        reconcile(&mut cache, &[slot(0, 10), restacked], 2.0);
        assert_eq!(order(&cache), [(0, 10), (1, 20)], "position unchanged");
        assert_eq!(cache[1].stacks, 5, "stack count refreshed");
        assert_eq!(cache[0].appeared_at, 1.0, "appeared_at is not disturbed");
    }

    /// The duration freshness gate (decision 0257 §3): a stamp older than the aura is ignored — the
    /// stale-recycled-slot defence — while a stamp from around the apply is accepted.
    #[test]
    fn a_duration_is_joined_only_when_it_is_no_older_than_the_aura() {
        // A stamp received at t=100 is stale for an aura that only appeared at t=200 — it belonged
        // to the slot's previous, since-expired occupant.
        let stale = DurationStamp {
            total: 30.0,
            expires_at: 130.0,
            received_at: 100.0,
        };
        assert_eq!(
            join_duration(Some(&stale), 200.0, 200.0, 5000.0),
            (0.0, 0.0),
            "a stamp seconds older than the aura is rejected — no timer, not a wrong one"
        );
        // A stamp received just before the aura appeared (the real apply→descriptor lead) joins,
        // and its expiry is rebased from the Bevy clock onto the script clock Lua counts against.
        let fresh = DurationStamp {
            total: 30.0,
            expires_at: 230.0,
            received_at: 199.9,
        };
        assert_eq!(
            join_duration(Some(&fresh), 200.0, 200.0, 5000.0),
            (30.0, 5030.0)
        );
        // No stamp at all is a permanent aura: the wire sends no packet for one (decision 0257).
        assert_eq!(join_duration(None, 200.0, 200.0, 5000.0), (0.0, 0.0));
    }

    /// **Decision 0846's first defect, pinned.** `SMSG_UPDATE_AURA_DURATION` LEADS the descriptor
    /// delta that names its slot (0257 B6; live-measured at one client frame, ~50 ms, on a fresh
    /// apply). So at the instant the stamp arrives its slot is **empty** — and the feed used to run
    /// a per-frame "drop the stamps of unoccupied slots" pass, which deleted it sub-millisecond
    /// later, before the aura it belonged to could appear. Every freshly cast buff then showed no
    /// timer, forever, while a *refresh* (whose slot is already occupied) worked — the "sometimes"
    /// in the report.
    ///
    /// The invariant that replaced it: nothing removes a stamp on occupancy, and the join is what
    /// decides. Here the aura arrives a frame *after* its stamp, and must still get its timer.
    #[test]
    fn a_stamp_that_arrives_one_frame_before_its_aura_still_joins_it() {
        let mut durations = AuraDurations::default();
        // t = 15.10 — the packet lands. Nothing occupies slot 0 yet; the cache is empty.
        durations.set(0, 300_000, 15.10);
        let mut cache = Vec::new();
        assert!(cache.is_empty());
        // t = 15.15 — the next frame's descriptor delta names slot 0.
        reconcile(&mut cache, &[slot(0, 1126)], 15.15);

        let (total, expiry) = join_duration(
            durations.by_slot.get(&0),
            cache[0].appeared_at,
            15.15,
            900.0,
        );
        assert_eq!(total, 300.0, "the stamp survived the gap");
        // Script clock 900 + the 300 s that started 50 ms ago: the bar counts down from the
        // packet's instant, not from the descriptor delta that named the slot.
        assert!(
            (expiry - 1199.95).abs() < 1e-9,
            "expiry rebased onto the script clock, got {expiry}"
        );
    }
    // == Decision 1035 — `UnitBuff 0x519500`'s unit-level gate ==
    //
    // The B216 case: a stealthed Ridge Stalker carries TWO auras that both resolve to
    // `Ability_Stealth` — 5916 "Shadowstalker Stealth" in a POSITIVE slot (creature_addon) and
    // 22766 "Sneak" in a NEGATIVE slot (EventAI; vmangos files it harmful because its
    // MOD_DECREASE_SPEED effect is negative). We drew both. The reference draws only the debuff,
    // because `UnitBuff` never enumerates a hostile creature's positive half at all.

    /// `UNIT_FIELD_FLAGS` index — the fixture writes it directly (`from_pairs` is the public
    /// fixture constructor; live code only ever decodes the wire).
    const F_FLAGS: u16 = 46;
    /// `UNIT_FIELD_CHARMEDBY`'s low dword — a guid pair, so the high half is the next index.
    const F_CHARMEDBY: u16 = 10;

    /// The B216 mob's two slots: 5916 helpful, 22766 harmful.
    fn ridge_stalker_slots(flags: u32) -> ObjectStore {
        ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
            (F_FLAGS, flags),
            (47, 5916),            // UNIT_FIELD_AURA slot 0 — the positive half
            (47 + 32, 22766),      // slot 32 — the negative half
            (95, 0x0000_0008),     // AURAFLAGS slot 0 nibble 0x8 (eff0)
            (95 + 4, 0x0000_0008), // slot 32 nibble 0x8
        ]))
    }

    /// **The gate's consequence, both ways.** With buffs denied, `other_unit_auras` returns the
    /// harmful half ONLY — which is what makes `UnitBuff("target", i)` nil for every i while
    /// `UnitDebuff` is untouched, exactly as the reference behaves (a single nil for every buff
    /// index, no slot examined). With buffs allowed, both come back.
    #[test]
    fn the_unit_gate_drops_the_whole_helpful_half_and_never_the_harmful_one() {
        let store = ridge_stalker_slots(0);

        let denied = other_unit_auras(&store, None, false);
        assert_eq!(
            denied.iter().map(|a| a.spell_id).collect::<Vec<_>>(),
            [22766],
            "a hostile creature shows its debuffs and NOT its buffs — the B216 divergence"
        );
        assert!(denied.iter().all(|a| !a.helpful));

        let allowed = other_unit_auras(&store, None, true);
        assert_eq!(
            allowed.iter().map(|a| a.spell_id).collect::<Vec<_>>(),
            [5916, 22766],
            "with the gate open both halves enumerate, ascending slot"
        );
    }

    /// **Detect Magic / GM mode short-circuits the gate.** `UNIT_FLAG_AURAS_VISIBLE` is clause
    /// one, tested before any reaction work — and it is why the same mob reads two ways: vmangos
    /// ORs the bit in for GameMasters only (`Object.cpp:765-766`), so a GM sees the buffs and an
    /// ordinary player does not. Nothing here consults GM state; the flag arrives on the wire.
    #[test]
    fn detect_magic_or_gm_mode_opens_the_gate_on_the_flag_alone() {
        let reputations = Reputations::default();
        // No factions catalog → `ring_reaction` falls through to NEUTRAL (3), which FAILS the
        // `>= 4` bar. So the flag is the only thing that can open the gate in this fixture.
        let hostile = ridge_stalker_slots(0);
        assert!(
            !buffs_visible_on(&hostile, None, None, &reputations, |_| None),
            "neutral reaction, not player-controlled, no PvP flag → no buffs"
        );

        let detected = ridge_stalker_slots(UNIT_FLAG_AURAS_VISIBLE);
        assert!(
            buffs_visible_on(&detected, None, None, &reputations, |_| None),
            "UNIT_FLAG_AURAS_VISIBLE alone opens it, whatever the reaction says"
        );
    }

    /// **`NOT_SELECTABLE` and the charm clause.** Both are outside the reaction ladder: the first
    /// is `CanAssist`'s own first test, the second is `UnitBuff`'s `activePlayer.CHARMEDBY == 0`
    /// (`0x5195f1`-`0x5195f7`) — while something else drives you, you assist nobody.
    #[test]
    fn not_selectable_and_being_charmed_each_close_the_gate_on_their_own() {
        let reputations = Reputations::default();

        // AURAS_VISIBLE would open it — but it is clause one and NOT_SELECTABLE lives inside
        // CanAssist, so the flag still wins. Pin that ordering, it is the reference's.
        let both = ridge_stalker_slots(UNIT_FLAG_AURAS_VISIBLE | (1 << 25));
        assert!(
            buffs_visible_on(&both, None, None, &reputations, |_| None),
            "AURAS_VISIBLE is tested FIRST and short-circuits CanAssist entirely"
        );

        // Player-controlled + friendly would otherwise pass the permissive arm; being charmed
        // ourselves closes it before CanAssist runs at all.
        let friendly_pc = ridge_stalker_slots(0x8);
        let charmed_self = ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
            (F_CHARMEDBY, 0x1234),
            (F_CHARMEDBY + 1, 0),
        ]));
        assert!(
            !buffs_visible_on(
                &friendly_pc,
                Some(&charmed_self),
                None,
                &reputations,
                |_| None
            ),
            "activePlayer.CHARMEDBY != 0 → no buffs on anyone"
        );
    }

    /// [`until_cancelled`] — `GetPlayerBuff`'s second return, against the two clauses the reference
    /// derives it from (`0x4e452e`-`0x4e45c5`). Every case here is one the corpus reads as a
    /// **number** compared to `1` (`CT_BuffMod/CT_BuffFrame.lua:175,232` and three more), so a wrong
    /// answer shows or hides a countdown rather than erroring.
    #[test]
    fn until_cancelled_follows_the_duration_row_and_the_area_aura_override() {
        use benilla_formats::{SpellDisplay, SpellDuration};

        let row = |base_ms: i32, per_level_ms: i32| SpellDuration {
            base_ms,
            per_level_ms,
            max_ms: base_ms,
        };
        // The shapes the shipped file actually holds (asserted on the real DBC in
        // `benilla_formats::spells::duration`'s tests): row 21 `{-1, 0}` = permanent, row 30
        // `{1_800_000, 0}` = Frost Armor's 30 minutes, row 427 `{-600_000, 60_000}` = the one that
        // makes this a TWO-field test, since a negative base alone is not permanence.
        let permanent_row = row(-1, 0);
        let timed_row = row(1_800_000, 0);
        let scaling_row = row(-600_000, 60_000);

        let spell = |effects: [u32; 3]| SpellDisplay {
            effects,
            ..SpellDisplay::default()
        };
        let ordinary = spell([6, 0, 0]);
        // 0x23 = the persistent-area-aura effect, one of the four in the override set.
        let area = spell([0x23, 0, 0]);

        // Clause 1 — the DBC duration row decides, cancelable or not.
        assert!(!until_cancelled(
            Some(&ordinary),
            Some(&timed_row),
            true,
            0.0
        ));
        assert!(!until_cancelled(
            Some(&ordinary),
            Some(&timed_row),
            false,
            0.0
        ));
        assert!(until_cancelled(
            Some(&ordinary),
            Some(&permanent_row),
            true,
            0.0
        ));
        assert!(
            !until_cancelled(Some(&ordinary), Some(&scaling_row), true, 0.0),
            "a negative base with a positive per-level term is a SCALING duration, not permanence"
        );
        // No row at all is the reference's `mov ecx,1` branch — permanent.
        assert!(until_cancelled(Some(&ordinary), None, true, 0.0));

        // Clause 2 — the area-aura override applies ONLY to a non-cancelable aura (`test
        // [edi+0xa],0x1; jne` returns early at 0x4e4588). This asymmetry is the whole clause: the
        // same spell answers differently depending on the wire's cancelable nibble.
        assert!(until_cancelled(Some(&area), Some(&timed_row), false, 0.0));
        assert!(
            !until_cancelled(Some(&area), Some(&timed_row), true, 0.0),
            "a CANCELABLE aura skips the effect scan entirely"
        );

        // The stated inference: the catalog cannot resolve the spell, so answer from the joined
        // expiry — the only evidence left.
        assert!(until_cancelled(None, None, false, 0.0));
        assert!(!until_cancelled(None, None, false, 1234.0));
    }
}
