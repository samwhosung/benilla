//! The aura feed: the `UNIT_FIELD_AURA` blocks of the player, the target, the target's target and
//! the pet as the ordered [`AuraState`] lists the aura bindings read, with the player's durations.
//! The player's list keeps the reference cache's insertion order (`0xbc6040`, repacked by
//! `PlayerAuras_Update 0x4e4170`); any other unit's reads its descriptor by ascending slot.

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

/// The player's aura durations by raw slot, each `SMSG_UPDATE_AURA_DURATION` stamped with its
/// arrival: the reference's expiry array `0xbc5f68`. Only the session's end clears it, never an
/// empty slot, whose aura may be a frame behind its packet, nor a worldport. At most 48 entries.
#[derive(Resource, Default)]
pub(crate) struct AuraDurations {
    by_slot: HashMap<u8, DurationStamp>,
}

struct DurationStamp {
    /// Seconds: the full duration on an apply or refresh, only the remainder on a relog
    /// (`Map::ExistingPlayerLogin`) or a cast pushback (`Unit::DelaySpellAuraHolder`).
    total: f64,
    /// The real-clock instant it runs out: a server-sent span counts down in real seconds.
    expires_at: f64,
    /// The real-clock arrival, the freshness gate against a recycled slot.
    received_at: f64,
}

impl AuraDurations {
    /// Records one `SMSG_UPDATE_AURA_DURATION`.
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

/// A record of the player's cache (`0xbc6040`): the position persists, the fields refresh.
struct CachedAura {
    slot: u8,
    spell_id: u32,
    /// When the aura entered the cache, for the duration freshness gate.
    appeared_at: f64,
    flags: u8,
    level: u8,
    stacks: u8,
}

/// The player's aura cache: buffs and debuffs interleaved, in insertion order.
#[derive(Resource, Default)]
pub(crate) struct PlayerAuraCache {
    auras: Vec<CachedAura>,
}

impl PlayerAuraCache {
    /// The live aura spell ids, pre-fed to `ui_tooltip` for a buff-bar hover's `SetPlayerBuff`.
    pub(crate) fn spell_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.auras.iter().map(|a| a.spell_id)
    }
}

/// [`feed_auras`] fires `PLAYER_AURAS_CHANGED` and `UNIT_AURA` inline, so a feed whose state their
/// handlers re-read (the stance feed) runs `.before(AuraEvents)` or they read last frame's.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct AuraEvents;

/// Adds the aura feed and the `CancelUnitBuff` drain.
pub(crate) struct UiAuraPlugin;

impl Plugin for UiAuraPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AuraDurations>()
            .init_resource::<PlayerAuraCache>()
            .init_resource::<AuraFeedMemory>()
            // Feed before the VM dispatch, drain after it.
            .add_systems(Update, feed_auras.in_set(UnitFeed).in_set(AuraEvents))
            .add_systems(Update, drain_aura_cancels.after(UiInput))
            // Torn down only at the session's end, not with the avatar, which a worldport drops.
            .add_systems(OnExit(ClientState::InWorld), end_session_aura_state);
    }
}

/// `PlayerAuras_Update`'s passes: drop a record whose slot lost its spell, closing the gap
/// (`0x4e421b`), then append new slots in ascending order (`0x4e424e`-`0x4e4374`).
fn reconcile(cache: &mut Vec<CachedAura>, live: &[UnitAuraSlot], now: f64) {
    cache.retain(|c| {
        live.iter()
            .any(|a| a.slot == c.slot && a.spell_id == c.spell_id)
    });
    for a in live {
        if let Some(c) = cache.iter_mut().find(|c| c.slot == a.slot) {
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

/// What a frame repaints on; not the countdown, which the button polls.
type AuraProjection = (u32, u8, bool, Option<String>);

/// The feed's edge memory: per token, the projection last pushed. Behind a
/// [`crate::ui_script::VmMemo`], so a `/reload`'s new VM reads it fresh and `UNIT_AURA` re-fires.
#[derive(Resource, Default)]
struct AuraFeedMemory {
    /// What this VM was last told.
    vm: crate::ui_script::VmMemo<AuraFeedMemo>,
    /// When [`trace_timers`] last printed: host state, outliving any one VM.
    traced_at: f64,
}

/// The per-VM `UNIT_AURA` edge keys.
#[derive(Default)]
struct AuraFeedMemo {
    present: bool,
    last: Vec<AuraProjection>,
    /// The tracking spell: a tracking switch changes no list, but the minimap needs the event.
    tracking_last: Option<u32>,
    /// The target's guid and projection: the guid makes a switch between equal lists re-fire.
    /// The pet's and the target's target's are keyed the same way.
    target_last: Option<(u64, Vec<AuraProjection>)>,
    pet_last: Option<(u64, Vec<AuraProjection>)>,
    tot_last: Option<(u64, Vec<AuraProjection>)>,
}

/// The `BENILLA_AURA_TRACE` period in seconds; a set value that is not a positive number means 1 s.
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

/// One trace tick: each drawn aura's time left beside its button's text, the button polling
/// `GetPlayerBuffTimeLeft` every frame (`BuffFrame.lua:130`). Buffs fill `BuffButton0..15`,
/// debuffs `BuffButton16..23`.
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
        // The rendered duration text: the button keeps no expiry, so its text is the readback.
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

/// Another unit's auras as `UnitBuff 0x519500` and `UnitDebuff 0x5198f0` read them: ascending
/// slot within each half, no durations, through the display filter. `buffs_visible` false drops
/// the helpful half, the unit-level gate's nil at every index ([`buffs_visible_on`]).
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
                // The 1.12 wire carries durations for the player alone.
                duration: 0.0,
                expiration_time: 0.0,
                helpful: a.slot < UNIT_AURA_POSITIVE_SLOTS,
                cancelable: a.flags & AURA_FLAG_CANCELABLE != 0,
                // `untilCancelled` is the player cache record's alone (`0xbc6040` `+0xc`).
                until_cancelled: false,
                channeled: display
                    .is_some_and(|d| d.attributes_ex & SPELL_ATTR_EX_IS_CHANNELED != 0),
            }
        })
        .collect()
}

/// `UnitBuff 0x519500`'s unit-level gate, before any slot: a unit's buffs are listed only if it
/// has `UNIT_FLAG_AURAS_VISIBLE` or the uncharmed player can assist it (`0x5195aa`-`0x5195f7`),
/// else every index is nil; `UnitDebuff` has no gate. vmangos sets the flag for Detect Magic
/// (`UnitDefines.h:572`) and for a GM viewer (`Object.cpp:765-766`), so it is read as received.
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
    // While charmed you assist nobody (`0x5195f1`-`0x5195f7`).
    if self_store.is_some_and(|s| s.0.unit_charmed_by().is_some()) {
        return false;
    }
    can_assist(Some(store), factions, reputations, self_store, owner_store)
}

/// `UNIT_FIELD_FLAGS` bit 27, `0x08000000` (`0x5195b6`).
const UNIT_FLAG_AURAS_VISIBLE: u32 = 0x0800_0000;

/// `AttributesEx & 0x4` (vmangos `SpellDefines.h:834`), the cancel gate's DBC arm (`0x4e4a10`).
const SPELL_ATTR_EX_IS_CHANNELED: u32 = 0x4;

/// `GetPlayerBuff`'s second return, cache record `+0xc`, set by `BuildBuffRecord 0x4e44b0` when
/// `DurationIndex` names an infinite `SpellDuration.dbc` row, base < 0 and per level <= 0
/// (`0x4e456e`-`0x4e457a`), or no row (`0x4e4580`); or when the aura is not cancelable and an
/// effect is an area aura, `0x23`, `0x77`, `0x80` or `0x81` (`0x4e4588`-`0x4e45be`).
/// `BuffFrame.lua:124` then shows no countdown. Deviation: on a catalog miss the reference keeps
/// the record's stale value (`0x4e4533`, `0x4e4550` skip the write); this answers from the joined
/// expiry, because a stale value is no mechanism to reproduce.
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
        // A negative, out-of-range or empty `DurationIndex`: all three reach `0x4e4580`.
        None => true,
    };
    permanent
        || (!cancelable
            && d.effects
                .iter()
                .any(|e| matches!(e, 0x23 | 0x77 | 0x80 | 0x81)))
}

/// The filter of every aura display, the player's bar and other units' rows alike: never-display
/// and tracking spells are hidden. The player's rebuild tests the attributes
/// (`0x4e42b6`-`0x4e42c8`), then diverts a tracking aura to the tracking global instead of the
/// cache (`0x4e42d6`-`0x4e4308`); `IsAuraDisplayable 0x519860` hides both from other units' rows.
/// Deviation: on other units' rows an id the catalog lacks stays visible, because every wire id is
/// a real spell and failing closed would blank every aura on a catalog failure; the reference
/// skips one there (`[0xc0d78c]`), though its player cache inserts one, as here.
fn shown_in_aura_ui(catalog: Option<&SpellCatalog>, spell_id: u32) -> bool {
    catalog
        .and_then(|c| c.get(spell_id))
        .is_none_or(|d| !d.hidden_from_aura_bar() && !d.tracking_aura())
}

/// The player's tracking aura, the reference's global `0xbc6378`: the rebuild's ascending walk
/// overwrites it per tracking aura, so the last wins (`0x4e4302`), after the attribute test; a
/// catalog miss cannot be told apart and goes to the bar. Read by `GetTrackingTexture`,
/// `CancelTrackingBuff` and `GameTooltip:SetTrackingSpell` ([`UiScript::set_tracking`]).
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
    pet: Res<crate::ui_pet::PetBar>,
    index: Res<GuidIndex>,
    // The `UnitBuff` gate's inputs, the same reaction the selection ring and nameplates use.
    factions: Option<Res<Factions>>,
    reputations: Res<Reputations>,
    // Read-only: the net path writes the stamps and only the session's end drops them.
    durations: Res<AuraDurations>,
    mut cache: ResMut<PlayerAuraCache>,
    time: Res<Time<Real>>,
    mut mem: ResMut<AuraFeedMemory>,
) {
    let Some(mut script) = script else {
        return;
    };
    let Ok((store, self_guid)) = self_q.single() else {
        // No avatar does not end the aura state: a worldport re-streams the avatar mid-session, and
        // vmangos re-sends durations only on apply, refresh and `Map::ExistingPlayerLogin`. The
        // reference's `0xbc5f68` is zeroed once, by the startup init (`0x4e40c0` from `0x48f5a9`),
        // and touched otherwise only by its setter `0x4e43a6` and reader `0x4e4467`.
        return;
    };

    let bevy_now = time.elapsed_secs_f64();
    let catalog = spells.as_ref().map(|s| &s.catalog);
    let spell_durations = spells.as_ref().map(|s| &s.durations);
    let occupied: Vec<UnitAuraSlot> = store.0.unit_auras().collect();
    // Filtered before the cache, so a hidden aura takes no position (`0x4e42b6`-`0x4e4308`).
    let live: Vec<UnitAuraSlot> = occupied
        .iter()
        .filter(|a| shown_in_aura_ui(catalog, a.spell_id))
        .copied()
        .collect();
    reconcile(&mut cache.auras, &live, bevy_now);

    // No stamp is pruned for an empty slot: the packet arrives before its slot fills.

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

    // Before the memory update, so a tracking change joins the edge key.
    let tracking = tracking_state_of(catalog, &occupied);
    let tracking_spell = tracking.as_ref().map(|t| t.spell_id);

    if let Some(period) = trace_period() {
        if bevy_now - mem.traced_at >= period {
            mem.traced_at = bevy_now;
            trace_timers(&script, &cache.auras, &list, bevy_now);
        }
    }

    // This VM's keys: a `/reload`'s new VM starts empty, so every edge re-fires.
    let memo = mem.vm.get(&script);

    // Fire on a discrete change, never on the countdown; a tracking switch counts.
    let projection = projection_of(&list);
    let changed = !memo.present || projection != memo.last || tracking_spell != memo.tracking_last;
    memo.last = projection;
    memo.tracking_last = tracking_spell;
    memo.present = true;

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

    // The target's rows. A self-target mirrors the player list in cache order, where the
    // reference's `UnitBuff` reads by slot.
    let target_list: Option<Vec<AuraState>> =
        selection.target.zip(selection.guid).and_then(|(e, guid)| {
            if guid == self_guid.0 {
                return Some(list.clone());
            }
            // Not `store`: that is the player's, the gate's second argument.
            let target_store = stores.get(e).ok()?;
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

    // The pet's list: the stock pet frame's four buttons draw its buffs (`PetFrame.lua:37,56`).
    let pet_guid = pet.spells.pet_guid;
    let pet_list: Option<Vec<AuraState>> = (pet_guid != 0)
        .then(|| index.0.get(&pet_guid))
        .flatten()
        .and_then(|&e| stores.get(e).ok())
        // Ungated; the reference gates a pet too, but `CanAssist`'s player-controlled arm
        // (`0x60673e`-`0x60679f`, the controlling players' duel and FFA-PvP state) passes for
        // your own pet, leaving only the charm, selectable and reaction clauses to hide its buffs.
        .map(|store| other_unit_auras(store, catalog, true));

    // The target's target's list, for the ToT frame's four debuff buttons, through the one-hop
    // `UNIT_FIELD_TARGET` read. When that is you, it is the player list, as for a self-target.
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
    // Clearing a token fires nothing: the frames react to `PLAYER_TARGET_CHANGED` and `UNIT_PET`.
    script.set_auras("target", target_list);
    script.set_auras("pet", pet_list);
    script.set_auras("targettarget", tot_list);
    script.set_tracking(tracking);
    if changed {
        script.fire_event("UNIT_AURA", vec![ScriptValue::Str("player".into())]);
        // The cache rebuild's own event, no arguments (`0x4e437f`): the stock buff bar and
        // `MiniMapTrackingFrame` register it, the unit frames `UNIT_AURA`.
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

/// How long before its aura a duration packet may arrive and still join it. The measured lead is
/// one frame, about 50 ms, on a fresh apply; a recycled slot's stale stamp is seconds older.
const DURATION_SLACK: f64 = 1.0;

/// Joins a slot's stamp to the aura in that slot as `(duration, expirationTime)` on the script
/// clock, `(0, 0)` for none; only this rejects a stamp. A stamp older than the aura by more than
/// [`DURATION_SLACK`] is dropped as an earlier occupant's, where the reference's reader
/// (`0x4e4450`) has no freshness test.
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

/// The session's end: drops the lists and stamps, so nothing reaches the next character's bar.
/// Deviation: the reference never clears its expiry array (`0xbc5f68`); this does, because vmangos
/// sends no durations at login to overwrite a kept stamp (`SMSG_UPDATE_AURA_DURATION` is an empty
/// placeholder in `Player::SendInitialPacketsBeforeAddToMap`, `Player.cpp:19248`).
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
    // No `AuraFeedMemory` reset: its edge keys die with the VM.
}

/// A `CMSG_CANCEL_AURA` per queued `CancelUnitBuff`, naming the spell, not the slot (`0x6e7040`).
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

    /// The plugin on a bare app: without a `UiScript` the systems that need one skip.
    fn aura_app() -> App {
        let (tx, _) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .insert_state(ClientState::InWorld)
            .init_resource::<Selection>()
            .init_resource::<crate::ui_pet::PetBar>()
            .init_resource::<GuidIndex>()
            // A required system parameter, unlike `Factions`.
            .init_resource::<Reputations>()
            .insert_resource(NetCommands(tx))
            .add_plugins(UiAuraPlugin);
        app
    }

    /// One live aura with a running timer: what a teleport must keep and a logout must drop.
    fn seed_one_timed_aura(app: &mut App) {
        app.world_mut()
            .resource_mut::<AuraDurations>()
            .set(32, 300_000, 10.0);
        app.world_mut()
            .resource_mut::<PlayerAuraCache>()
            .auras
            .push(CachedAura {
                slot: 32, // the first harmful slot, a debuff
                spell_id: 11976,
                appeared_at: 10.0,
                flags: 0x8,
                level: 60,
                stacks: 1,
            });
    }

    /// A worldport's avatar-less frames keep every aura live, and no duration is re-sent then.
    #[test]
    fn the_aura_state_survives_an_avatar_less_frame_and_dies_only_with_the_session() {
        let mut app = aura_app();
        seed_one_timed_aura(&mut app);

        // Frames with no avatar: the worldport gap.
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

        // The session ends: `/logout` to the glue screens.
        app.world_mut()
            .resource_mut::<NextState<ClientState>>()
            .set(ClientState::CharSelect);
        app.update();
        assert!(app.world().resource::<PlayerAuraCache>().auras.is_empty());
        assert!(app.world().resource::<AuraDurations>().by_slot.is_empty());
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

    /// Against the real `Spell.dbc`; skips without client data.
    #[test]
    fn the_aura_display_filter_hides_a_real_battle_stance_but_keeps_battle_shout() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let catalog = benilla_formats::load_spell_catalog(&mut chain).expect("Spell.dbc");

        // Battle Stance 2457 and Battle Shout 6673.
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

    /// Against the real `Spell.dbc` (`0x4e42d6`-`0x4e4308`); skips without client data.
    #[test]
    fn a_real_tracking_aura_is_diverted_from_the_bar_to_the_tracking_state() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let catalog = benilla_formats::load_spell_catalog(&mut chain).expect("Spell.dbc");

        // Find Herbs 2383 (aura 45), Battle Shout 6673, Track Beasts 1494 (aura 44).
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

        // The last tracking aura in slot order holds the global.
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

        // No tracking aura, or no catalog to identify one: no tracking state.
        assert!(tracking_state_of(Some(&catalog), &[slot(0, 6673)]).is_none());
        assert!(tracking_state_of(None, &slots).is_none());
    }

    #[test]
    fn a_new_low_slot_aura_appends_at_the_end_not_sorted_by_slot() {
        let mut cache = Vec::new();
        // X lands in slot 5 first.
        reconcile(&mut cache, &[slot(5, 100)], 1.0);
        assert_eq!(order(&cache), [(5, 100)]);
        // Y lands in slot 2 later: the descriptor reads [2, 5], but Y is newer.
        reconcile(&mut cache, &[slot(2, 200), slot(5, 100)], 2.0);
        assert_eq!(
            order(&cache),
            [(5, 100), (2, 200)],
            "insertion order, not ascending slot"
        );
    }

    /// `PlayerAuras_Update`'s shift-down (`0x4e421b`), then a recycled slot's append.
    #[test]
    fn a_dropped_aura_repacks_and_a_recycled_slot_appends_fresh() {
        let mut cache = Vec::new();
        reconcile(&mut cache, &[slot(0, 10), slot(1, 20), slot(2, 30)], 1.0);
        assert_eq!(order(&cache), [(0, 10), (1, 20), (2, 30)]);

        // The middle aura (slot 1) drops.
        reconcile(&mut cache, &[slot(0, 10), slot(2, 30)], 2.0);
        assert_eq!(order(&cache), [(0, 10), (2, 30)], "the gap closes");

        // A new spell takes slot 1: it appends, not back into the old middle.
        reconcile(&mut cache, &[slot(0, 10), slot(1, 99), slot(2, 30)], 3.0);
        assert_eq!(
            order(&cache),
            [(0, 10), (2, 30), (1, 99)],
            "the recycled slot is the newest, so it is last"
        );
    }

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

    #[test]
    fn a_duration_is_joined_only_when_it_is_no_older_than_the_aura() {
        // Received at t=100 for an aura that appeared at t=200: a previous occupant's.
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
        // Received just before the aura: joined, and rebased onto the script clock.
        let fresh = DurationStamp {
            total: 30.0,
            expires_at: 230.0,
            received_at: 199.9,
        };
        assert_eq!(
            join_duration(Some(&fresh), 200.0, 200.0, 5000.0),
            (30.0, 5030.0)
        );
        // No stamp: a permanent aura, which gets no packet.
        assert_eq!(join_duration(None, 200.0, 200.0, 5000.0), (0.0, 0.0));
    }

    /// The duration packet arrives about a frame before the delta that fills its slot.
    #[test]
    fn a_stamp_that_arrives_one_frame_before_its_aura_still_joins_it() {
        let mut durations = AuraDurations::default();
        // t = 15.10: the packet lands while slot 0 is still empty.
        durations.set(0, 300_000, 15.10);
        let mut cache = Vec::new();
        assert!(cache.is_empty());
        // t = 15.15: the next frame's descriptor delta fills slot 0.
        reconcile(&mut cache, &[slot(0, 1126)], 15.15);

        let (total, expiry) = join_duration(
            durations.by_slot.get(&0),
            cache[0].appeared_at,
            15.15,
            900.0,
        );
        assert_eq!(total, 300.0, "the stamp survived the gap");
        // Script clock 900 plus the 300 s that began 50 ms ago, at the packet, not the delta.
        assert!(
            (expiry - 1199.95).abs() < 1e-9,
            "expiry rebased onto the script clock, got {expiry}"
        );
    }
    // == `UnitBuff`'s unit-level gate (`0x519500`) ==
    // A stealthed Ridge Stalker carries 5916 in a positive slot and 22766 in a negative one; the
    // reference draws only 22766, as `UnitBuff` lists no hostile creature's buffs.

    /// `UNIT_FIELD_FLAGS`'s index.
    const F_FLAGS: u16 = 46;
    /// `UNIT_FIELD_CHARMEDBY`'s low dword; the high half is the next index.
    const F_CHARMEDBY: u16 = 10;

    /// The Ridge Stalker's two slots: 5916 helpful, 22766 harmful.
    fn ridge_stalker_slots(flags: u32) -> ObjectStore {
        ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
            (F_FLAGS, flags),
            (47, 5916),            // UNIT_FIELD_AURA slot 0, the positive half
            (47 + 32, 22766),      // slot 32, the negative half
            (95, 0x0000_0008),     // AURAFLAGS slot 0 nibble 0x8 (eff0)
            (95 + 4, 0x0000_0008), // slot 32 nibble 0x8
        ]))
    }

    /// Denied, `UnitBuff` is nil at every index; `UnitDebuff` is untouched.
    #[test]
    fn the_unit_gate_drops_the_whole_helpful_half_and_never_the_harmful_one() {
        let store = ridge_stalker_slots(0);

        let denied = other_unit_auras(&store, None, false);
        assert_eq!(
            denied.iter().map(|a| a.spell_id).collect::<Vec<_>>(),
            [22766],
            "a hostile creature shows its debuffs and NOT its buffs — `UnitBuff`'s unit gate"
        );
        assert!(denied.iter().all(|a| !a.helpful));

        let allowed = other_unit_auras(&store, None, true);
        assert_eq!(
            allowed.iter().map(|a| a.spell_id).collect::<Vec<_>>(),
            [5916, 22766],
            "with the gate open both halves enumerate, ascending slot"
        );
    }

    /// vmangos sets `UNIT_FLAG_AURAS_VISIBLE` for a GM viewer too (`Object.cpp:765-766`).
    #[test]
    fn detect_magic_or_gm_mode_opens_the_gate_on_the_flag_alone() {
        let reputations = Reputations::default();
        // No factions catalog: the reaction is neutral (3), below the `>= 4` bar.
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

    /// `NOT_SELECTABLE` is `CanAssist`'s; the charm clause is `UnitBuff`'s (`0x5195f1`-`0x5195f7`).
    #[test]
    fn not_selectable_and_being_charmed_each_close_the_gate_on_their_own() {
        let reputations = Reputations::default();

        // AURAS_VISIBLE is tested before `CanAssist`, so it wins over NOT_SELECTABLE.
        let both = ridge_stalker_slots(UNIT_FLAG_AURAS_VISIBLE | (1 << 25));
        assert!(
            buffs_visible_on(&both, None, None, &reputations, |_| None),
            "AURAS_VISIBLE is tested FIRST and short-circuits CanAssist entirely"
        );

        // Being charmed closes the gate before `CanAssist` runs.
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

    /// The two clauses of `0x4e452e`-`0x4e45c5`.
    #[test]
    fn until_cancelled_follows_the_duration_row_and_the_area_aura_override() {
        use benilla_formats::{SpellDisplay, SpellDuration};

        let row = |base_ms: i32, per_level_ms: i32| SpellDuration {
            base_ms,
            per_level_ms,
            max_ms: base_ms,
        };
        // Real `SpellDuration.dbc` rows: 21 `{-1, 0}` permanent, 30 `{1_800_000, 0}` 30 minutes,
        // 427 `{-600_000, 60_000}`, whose negative base alone is not permanence.
        let permanent_row = row(-1, 0);
        let timed_row = row(1_800_000, 0);
        let scaling_row = row(-600_000, 60_000);

        let spell = |effects: [u32; 3]| SpellDisplay {
            effects,
            ..SpellDisplay::default()
        };
        let ordinary = spell([6, 0, 0]);
        // 0x23 is `SPELL_EFFECT_APPLY_AREA_AURA_PARTY`, one of the four in the override set.
        let area = spell([0x23, 0, 0]);

        // Clause 1: the duration row decides, cancelable or not.
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
        // No row: permanent (`0x4e4580`).
        assert!(until_cancelled(Some(&ordinary), None, true, 0.0));

        // Clause 2: the area-aura override applies only to a non-cancelable aura (`0x4e4588`).
        assert!(until_cancelled(Some(&area), Some(&timed_row), false, 0.0));
        assert!(
            !until_cancelled(Some(&area), Some(&timed_row), true, 0.0),
            "a CANCELABLE aura skips the effect scan entirely"
        );

        // A catalog miss answers from the joined expiry.
        assert!(until_cancelled(None, None, false, 0.0));
        assert!(!until_cancelled(None, None, false, 1234.0));
    }
}
