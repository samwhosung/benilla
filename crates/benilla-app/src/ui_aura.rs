//! The aura feed: the `UNIT_FIELD_AURA` blocks of the player and of every unit a token can name as
//! the ordered [`AuraState`] lists the aura bindings read, with the player's durations, and the
//! unit-token resolver's inputs the bindings reach them through ([`units`]). The player's list
//! keeps the reference cache's insertion order (`0xbc6040`, repacked by `PlayerAuras_Update
//! 0x4e4170`); any other unit's reads its descriptor by ascending slot, or, for a group member or
//! pet with no live object, its roster record.

mod units;

use std::collections::HashMap;

use bevy::prelude::*;

use benilla_formats::SpellCatalog;
use benilla_protocol::messages::{UnitAuraSlot, AURA_FLAG_CANCELABLE, UNIT_AURA_POSITIVE_SLOTS};
use benilla_ui::script::{AuraState, TrackingState, UiScript};

use crate::client_state::ClientState;
use crate::net::{ClientCommand, Guid, GuidIndex, NetCommands, ObjectStore, SelfPlayer};
use crate::target::{RelUnit, Relations, Selection};
use crate::ui_action::Spells;
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
    /// Records one `SMSG_UPDATE_AURA_DURATION`. The reference stores `now + ms` over the slot's
    /// expiry (`0x4e4390`) and its reader answers 0 once `now - expiry >= 0` (`0x4e4472`), so a
    /// negative span is a lapsed timer that replaces the slot's last one; here that is no stamp,
    /// which every reader answers `(0, 0)`. The one exception is `i32::MIN`: the wrapped
    /// difference `now - (now + 0x80000000)` is itself negative, so the `js` is taken and the
    /// reader returns `expiry - now`, 2^31 ms (24.8 days) left, shrinking with the clock.
    pub(crate) fn set(&mut self, slot: u8, remaining_ms: i32, now: f64) {
        if trace_period().is_some() {
            info!(
                "aura trace: SMSG_UPDATE_AURA_DURATION slot {slot} = {remaining_ms} ms @ {now:.2}"
            );
        }
        if remaining_ms < 0 && remaining_ms != i32::MIN {
            self.by_slot.remove(&slot);
            return;
        }
        // `i32::MIN` reads back as the unsigned 2^31.
        let total = f64::from(remaining_ms.unsigned_abs()) / 1000.0;
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

/// A record of the player's cache (`0xbc6040`), made by `BuildBuffRecord 0x4e44b0` when its aura
/// enters: the position persists, and only the stack count refreshes.
struct CachedAura {
    slot: u8,
    spell_id: u32,
    /// When the aura entered the cache, for the duration freshness gate.
    appeared_at: f64,
    /// The `AURAFLAGS` nibble and the `AURALEVELS` byte as the record was made (`+0xa`, `+0x8`).
    flags: u8,
    level: u8,
    stacks: u8,
}

/// The player's aura cache: buffs and debuffs interleaved, in insertion order.
#[derive(Resource, Default)]
pub(crate) struct PlayerAuraCache {
    auras: Vec<CachedAura>,
}

/// [`feed_auras`] fires `PLAYER_AURAS_CHANGED` inline and seats the unit tokens the field events
/// fan out over, so a feed whose state their handlers re-read (the stance feed) runs
/// `.before(AuraEvents)` or they read last frame's.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct AuraEvents;

/// Adds the aura feed and the `CancelPlayerBuff`/`CancelTrackingBuff` drain.
pub(crate) struct UiAuraPlugin;

impl Plugin for UiAuraPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AuraDurations>()
            .init_resource::<PlayerAuraCache>()
            .init_resource::<AuraFeedMemory>()
            .init_resource::<benilla_world::frame_pace::FrameStep>()
            // Feed before the VM dispatch, drain after it.
            // After this frame's world publish, so the gain names the unit in the same frame.
            .add_systems(
                Update,
                feed_auras
                    .in_set(UnitFeed)
                    .in_set(AuraEvents)
                    .after(crate::ui_tooltip::MouseoverPublish),
            )
            .add_systems(Update, drain_aura_cancels.in_set(crate::ui_script::UiDrain))
            // Torn down only at the session's end, not with the avatar, which a worldport drops.
            .add_systems(OnExit(ClientState::InWorld), end_session_aura_state);
    }
}

/// `PlayerAuras_Update`'s passes: drop a record whose slot lost its spell, closing the gap
/// (`0x4e421b`), then append new slots in ascending order (`0x4e424e`-`0x4e4374`). A record kept
/// across passes refreshes its stack count alone (`0x4e433b`-`0x4e434a`).
fn reconcile(cache: &mut Vec<CachedAura>, live: &[UnitAuraSlot], now: f64) {
    cache.retain(|c| {
        live.iter()
            .any(|a| a.slot == c.slot && a.spell_id == c.spell_id)
    });
    for a in live {
        if let Some(c) = cache.iter_mut().find(|c| c.slot == a.slot) {
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

/// The feed's memory of what each VM holds. Behind a [`crate::ui_script::VmMemo`], so a
/// `/reload`'s new VM reads it fresh and `PLAYER_AURAS_CHANGED` re-fires.
#[derive(Resource, Default)]
struct AuraFeedMemory {
    /// What this VM was last told.
    vm: crate::ui_script::VmMemo<AuraFeedMemo>,
    /// The `raidFilter` test's books, rebuilt when either book or the catalog moves.
    raid: RaidFilter,
    /// When [`trace_timers`] last printed: host state, outliving any one VM.
    traced_at: f64,
}

/// One VM's: the player's projection, `PLAYER_AURAS_CHANGED`'s edge key, and every other list.
#[derive(Default)]
struct AuraFeedMemo {
    present: bool,
    last: Vec<AuraProjection>,
    /// The tracking spell: a tracking switch changes no list, but the minimap needs the event.
    tracking_last: Option<u32>,
    /// Every other unit's list, by guid, the resolver's inputs and their pushes.
    units: units::UnitAuras,
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

/// One aura as `UnitBuff 0x519500` and `UnitDebuff 0x5198f0` return it for a unit not the player:
/// no duration, which the 1.12 wire carries for the player alone.
fn unit_aura_state(catalog: Option<&SpellCatalog>, input: &AuraInput) -> AuraState {
    let &AuraInput {
        slot,
        spell_id,
        count,
        flags,
        level,
        raid_listed,
    } = input;
    let display = catalog.and_then(|cat| cat.get(spell_id));
    AuraState {
        spell_id,
        name: display.map(|d| d.name.clone()),
        icon: display.and_then(|d| d.icon.clone()),
        count,
        level,
        debuff_type: display
            .zip(catalog)
            .and_then(|(d, cat)| cat.dispel_name(d))
            .map(str::to_string),
        duration: 0.0,
        expiration_time: 0.0,
        helpful: slot < UNIT_AURA_POSITIVE_SLOTS,
        cancelable: flags & AURA_FLAG_CANCELABLE != 0,
        // `untilCancelled` is the player cache record's alone (`0xbc6040` `+0xc`).
        until_cancelled: false,
        channeled: display.is_some_and(|d| d.attributes_ex & SPELL_ATTR_EX_IS_CHANNELED != 0),
        raid_listed,
    }
}

/// What one aura of a unit's list is built from: its slot, spell, stack count, `AURAFLAGS` nibble,
/// `AURALEVELS` byte and `raidFilter` verdict ([`RaidFilter`]). A list is a pure function of these
/// and the spell catalog.
#[derive(Clone, Copy, Debug, PartialEq)]
struct AuraInput {
    slot: u8,
    spell_id: u32,
    count: u8,
    flags: u8,
    level: u8,
    raid_listed: bool,
}

/// The list [`AuraInput`]s make.
fn aura_list(catalog: Option<&SpellCatalog>, inputs: &[AuraInput]) -> Vec<AuraState> {
    inputs
        .iter()
        .map(|input| unit_aura_state(catalog, input))
        .collect()
}

/// A unit's auras as `UnitBuff 0x519500`, `UnitDebuff 0x5198f0` and the tooltip's
/// `SetUnitBuff 0x534ac0` read them: ascending slot within each half, no durations, through the
/// display filter, every one whatever the unit gate says ([`buffs_visible_on`]), which the list
/// carries beside them, each with its `raidFilter` verdict.
fn other_unit_inputs<'a>(
    store: &'a ObjectStore,
    catalog: Option<&'a SpellCatalog>,
    raid: &'a RaidFilter,
) -> impl Iterator<Item = AuraInput> + 'a {
    store
        .0
        .unit_auras()
        .filter(move |a| shown_in_aura_ui(catalog, a.spell_id))
        .map(move |a| AuraInput {
            slot: a.slot,
            spell_id: a.spell_id,
            count: a.stacks,
            flags: a.flags,
            level: a.level,
            raid_listed: raid.keeps(catalog, a.slot, a.spell_id),
        })
}

/// A roster record's aura block as the two bindings' roster walk reads it (`0x519763`-`0x5197d2`,
/// `0x519b53`-`0x519bc5`): ascending slot, the display filter alone (`0x51979f`, `0x519b8f`), no
/// unit gate and no `AURAFLAGS`, which the record does not hold. The stack count is always 1
/// (`0x51980f`, `0x519c2d`), nothing is cancelable, and the level is 0, what `SetUnitBuff` passes
/// for a record (`0x534da8`).
fn record_inputs<'a>(
    slots: impl Iterator<Item = (u8, u16)> + 'a,
    catalog: Option<&'a SpellCatalog>,
    raid: &'a RaidFilter,
) -> impl Iterator<Item = AuraInput> + 'a {
    slots
        .map(|(slot, spell)| (slot, u32::from(spell)))
        .filter(move |&(_, spell)| spell != 0 && shown_in_aura_ui(catalog, spell))
        .map(move |(slot, spell_id)| AuraInput {
            slot,
            spell_id,
            count: 1,
            flags: 0,
            level: 0,
            raid_listed: raid.keeps(catalog, slot, spell_id),
        })
}

/// What a set `raidFilter` keeps, the same test on the live walk and the record's (`UnitBuff`
/// `0x51967b`, `0x5197a8`; `UnitDebuff` `0x519a1a`; the tooltip's `0x534c6c`, `0x534d6b`,
/// `0x534fec`): a buff the player could cast (`0x4b3870`), a debuff the player can dispel
/// (`0x4b3920`). Built off the player's and the pet's books.
#[derive(Default)]
struct RaidFilter {
    /// The player's known spells, the bits at `[0xb710fc]` that every learn sets (`0x4b2603`).
    known: std::collections::HashSet<u32>,
    /// The pet's book, `0xb6f098`, which `0x4b3870` scans when the player does not know the spell
    /// (`0x4b38a0`).
    pet_book: Vec<u32>,
    /// The dispel list `0xb71118`: each learnt spell's `SPELL_EFFECT_DISPEL` (38) effects'
    /// `EffectMiscValue`, but an effect aimed at an enemy (`EffectImplicitTargetA` 6), appended at
    /// the learn (`0x4b26a2`-`0x4b26ee`).
    dispels: Vec<u32>,
}

impl RaidFilter {
    #[cfg(test)]
    fn build<'a>(
        known: impl Iterator<Item = &'a u32>,
        pet_book: impl Iterator<Item = u32>,
        catalog: Option<&SpellCatalog>,
    ) -> Self {
        let mut raid = Self::default();
        raid.set_known(known, catalog);
        raid.pet_book.extend(pet_book);
        raid
    }

    /// The player's book and the dispel list gathered from it.
    fn set_known<'a>(
        &mut self,
        known: impl Iterator<Item = &'a u32>,
        catalog: Option<&SpellCatalog>,
    ) {
        self.known.clear();
        self.known.extend(known);
        self.dispels.clear();
        for d in self.known.iter().filter_map(|&id| catalog?.get(id)) {
            for i in 0..3 {
                let kind = d.effect_misc_value[i] as u32;
                if d.effects[i] == SPELL_EFFECT_DISPEL
                    && d.effect_implicit_target_a[i] != TARGET_ENEMY
                    && !self.dispels.contains(&kind)
                {
                    self.dispels.push(kind);
                }
            }
        }
    }

    /// The verdict on the aura in `slot`: the buff test on the first 32, the debuff test after.
    fn keeps(&self, catalog: Option<&SpellCatalog>, slot: u8, spell_id: u32) -> bool {
        let Some(d) = catalog.and_then(|c| c.get(spell_id)) else {
            // Both tests read the row, which the walks require first (`0x519669`, `0x4b38ca`).
            return false;
        };
        if slot >= UNIT_AURA_POSITIVE_SLOTS {
            // `0x4b3920(Dispel)`, the row's `+0x10` (`0x519a1c`).
            return self.dispels.contains(&d.dispel);
        }
        // `0x4b3870`: known or in the pet's book (`0x4b3893`, `0x4b38a0`), then `AttributesEx3`
        // bit `0x4000` clear (`0x4b38cf`), the first effect not aimed at the caster
        // (`ImplicitTargetA` 1, `0x4b38d4`), and no effect of 35, 119, 128, 129 or 27, the party,
        // pet and area auras and the persistent area aura (`0x4b38e5`-`0x4b3902`).
        (self.known.contains(&spell_id) || self.pet_book.contains(&spell_id))
            && d.attributes_ex3 & 0x4000 == 0
            && d.effect_implicit_target_a[0] != TARGET_SELF
            && !d
                .effects
                .iter()
                .any(|e| matches!(e, 0x23 | 0x77 | 0x80 | 0x81 | 0x1b))
    }
}

/// `SPELL_EFFECT_DISPEL`, the effect the dispel list is gathered from (`0x4b26a2`).
const SPELL_EFFECT_DISPEL: u32 = 38;
/// `TARGET_UNIT_CASTER` (vmangos `SpellDefines.h:26`), the first effect's target that fails the
/// buff test (`0x4b38d4`).
const TARGET_SELF: u32 = 1;
/// `TARGET_UNIT_ENEMY` (`SpellDefines.h:31`): a dispel aimed there is an offensive one, left off
/// the dispel list (`0x4b26ab`).
const TARGET_ENEMY: u32 = 6;

/// `UnitBuff 0x519500`'s unit-level gate, before any slot: a unit's buffs are listed only if it
/// has `UNIT_FLAG_AURAS_VISIBLE` or the uncharmed player can assist it (`0x5195aa`-`0x5195f7`),
/// else every index is nil; `UnitDebuff` and `GameTooltip:SetUnitBuff` have no gate. vmangos sets
/// the flag for Detect Magic (`UnitDefines.h:572`) and for a GM viewer (`Object.cpp:765-766`), so
/// it is read as received.
fn buffs_visible_on(rel: &Relations, unit: &RelUnit, me: Option<&RelUnit>) -> bool {
    if unit.flags & UNIT_FLAG_AURAS_VISIBLE != 0 {
        return true;
    }
    // While charmed you assist nobody (`0x5195f1`-`0x5195f7`); with no player, nobody either.
    me.is_some_and(|me| me.charmed_by == 0 && rel.can_assist(me, unit))
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

/// The units the resolver's guid globals name, from `UnitTokens`' own sources; the mouseover is
/// the VM's published one.
#[derive(bevy::ecs::system::SystemParam)]
struct AuraUnits<'w> {
    selection: Res<'w, Selection>,
    pet: Res<'w, crate::ui_pet::PetBar>,
    // `Option`: a UI-only harness lacks their plugins; without the party's no group token names
    // anyone.
    group: Option<Res<'w, crate::ui_party::GroupState>>,
    interact: Option<Res<'w, crate::ui_session::InteractNpc>>,
}

fn feed_auras(
    mut vm: crate::script_calls::VmWith<AuraUnits<'static>>,
    self_q: Query<(&ObjectStore, &Guid), With<SelfPlayer>>,
    stores: Query<&ObjectStore>,
    spells: Option<Res<Spells>>,
    index: Res<GuidIndex>,
    // The `UnitBuff` gate's inputs, the relation model the selection ring and nameplates use.
    relations: crate::target::RelationWorld,
    // Read-only: the net path writes the stamps and only the session's end drops them.
    durations: Res<AuraDurations>,
    mut cache: ResMut<PlayerAuraCache>,
    time: Res<Time<Real>>,
    // The stamps' clock where `GetTime` stands: this feed runs in `UiFeed`, before the tick, so
    // the VM's clock is still the previous tick's.
    step: Res<benilla_world::frame_pace::FrameStep>,
    // `Option`: a UI-only harness has no action store, and so no known spell.
    actions: Option<Res<crate::ui_action::PlayerActions>>,
    mut mem: ResMut<AuraFeedMemory>,
) {
    vm.feed(|script, units, fires| {
        let Some(script) = script else {
            return;
        };
        let Ok((store, self_guid)) = self_q.single() else {
            // No avatar does not end the aura state: a worldport re-streams the avatar mid-session,
            // and vmangos re-sends durations only on apply, refresh and `Map::ExistingPlayerLogin`.
            // The reference's `0xbc5f68` is zeroed once, by the startup init (`0x4e40c0` from
            // `0x48f5a9`), and touched otherwise only by its setter `0x4e43a6` and reader
            // `0x4e4467`.
            return;
        };

        let mem = &mut *mem;
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
                    step.before_step().1,
                    script_now,
                );
                AuraState {
                    spell_id: c.spell_id,
                    name: display.map(|d| d.name.clone()),
                    icon: display.and_then(|d| d.icon.clone()),
                    count: c.stacks,
                    level: c.level,
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
                    // No reader of the buff cache takes a `raidFilter`.
                    raid_listed: false,
                }
            })
            .collect();

        // Before the memory update, so a tracking change joins the edge key.
        let tracking = tracking_state_of(catalog, &occupied);
        let tracking_spell = tracking.as_ref().map(|t| t.spell_id);

        if let Some(period) = trace_period() {
            if bevy_now - mem.traced_at >= period {
                mem.traced_at = bevy_now;
                trace_timers(script, &cache.auras, &list, bevy_now);
            }
        }

        let catalog_moved = spells.as_ref().is_some_and(|s| s.is_changed());
        if catalog_moved || actions.as_ref().is_some_and(|a| a.is_changed()) {
            mem.raid
                .set_known(actions.iter().flat_map(|a| a.spells.iter()), catalog);
        }
        if catalog_moved || units.pet.is_changed() {
            // The pet's book is the bar's spells its add-gate keeps (`0x4b2f90`, `ui_pet_book`).
            let pet = &units.pet;
            mem.raid.pet_book.clear();
            mem.raid.pet_book.extend(
                pet.spells
                    .spells
                    .iter()
                    .map(|e| e.action())
                    .filter(|&id| {
                        catalog
                            .and_then(|c| c.get(id))
                            .is_some_and(|d| d.in_pet_book())
                    })
                    .filter(|_| pet.has_bar()),
            );
        }
        // This VM's keys: a `/reload`'s new VM starts empty, so every edge re-fires.
        let memo = mem.vm.get(script);

        // Fire on a discrete change, never on the countdown; a tracking switch counts.
        let projection = projection_of(&list);
        let changed =
            !memo.present || projection != memo.last || tracking_spell != memo.tracking_last;
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

        let held = |guid: u64| index.0.get(&guid).and_then(|&e| stores.get(e).ok());
        let pet_guid = units.pet.spells.pet_guid;
        // `UnitBuff`'s unit gate on a live unit, the player included, whom the gate tests as any
        // unit (`0x5195aa`-`0x5195f7`). The pet is ungated; the reference gates a pet too, but
        // `CanAssist`'s player-controlled arm (`0x60673e`-`0x60679f`, the controlling players' duel
        // and FFA-PvP state) passes for your own pet, leaving only the charm, selectable and
        // reaction clauses to hide its buffs.
        let (rel, me) = (relations.get(), relations.me());
        let buffs_visible = |guid: u64, unit: &ObjectStore| {
            guid == pet_guid || buffs_visible_on(&rel, &relations.unit_of(guid, unit), me.as_ref())
        };
        let target_guid = units.selection.guid.unwrap_or(0);
        // The resolver reads the published pair `[0xb4e2c8]` (`0x515bb6`, `0x515bdb`), not the
        // pick, and takes it only as a unit (`0x515bca mov ecx,8`, `0x515bd9 je`): a GameObject, a
        // corpse or nothing names nobody.
        let mouseover = script
            .world_mouseover()
            .filter(|&guid| held(guid).is_some_and(|s| s.is_unit()))
            .unwrap_or(0);
        let unit_changed = memo.units.feed(
            script,
            units::Inputs {
                group: units.group.as_deref(),
                me: self_guid.0,
                pet: pet_guid,
                target: target_guid,
                mouseover,
                npc: units.interact.as_deref().and_then(|i| i.1).unwrap_or(0),
                held,
                buffs_visible,
                catalog,
                catalog_moved,
                raid: &mem.raid,
            },
        );

        if unit_changed.contains(&target_guid) && std::env::var_os("BENILLA_AURA_DUMP").is_some() {
            let l = memo.units.list(target_guid).map(|l| l.auras.as_slice());
            let l = l.unwrap_or_default();
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

        script.set_player_auras(list);
        script.set_tracking(tracking);
        if changed {
            // The cache rebuild's own event, no arguments (`0x4e437f`): the stock buff bar and
            // `MiniMapTrackingFrame` register it. The unit frames' `UNIT_AURA` is the aura fields'
            // watches (ids 41 and 107), the flags watcher's bit 27 and the stats packet's record
            // diff (`0x5e57d8`, the pet's `0x5e58f1`), all fired by `crate::ui_unit` after this
            // push; a token that comes to name another unit hears it from its own event, not from
            // here.
            fires.post("PLAYER_AURAS_CHANGED", vec![]);
        }
    })
}

/// How long before its aura a duration packet may arrive and still join it. The measured lead is
/// one frame, about 50 ms, on a fresh apply; a recycled slot's stale stamp is seconds older.
const DURATION_SLACK: f64 = 1.0;

/// Joins a slot's stamp to the aura in that slot as `(duration, expirationTime)` on the script
/// clock, `(0, 0)` for none; only this rejects a stamp. A stamp older than the aura by more than
/// [`DURATION_SLACK`] is dropped as an earlier occupant's, where the reference's reader
/// (`0x4e4450`) has no freshness test. `stamp_now` is the stamps' clock at the instant
/// `script_now` stands at, so the expiry holds still frame to frame.
fn join_duration(
    stamp: Option<&DurationStamp>,
    appeared_at: f64,
    stamp_now: f64,
    script_now: f64,
) -> (f64, f64) {
    stamp
        .filter(|d| d.received_at >= appeared_at - DURATION_SLACK)
        .map(|d| (d.total, script_now + (d.expires_at - stamp_now)))
        .unwrap_or((0.0, 0.0))
}

/// The session's end: drops the lists and stamps, so nothing reaches the next character's bar.
/// Deviation: the reference never clears its expiry array (`0xbc5f68`); this does, because vmangos
/// sends no durations at login to overwrite a kept stamp (`SMSG_UPDATE_AURA_DURATION` is an empty
/// placeholder in `Player::SendInitialPacketsBeforeAddToMap`, `Player.cpp:19248`).
fn end_session_aura_state(
    mut hosted_vm: crate::script_calls::HostedVm,
    mut durations: ResMut<AuraDurations>,
    mut cache: ResMut<PlayerAuraCache>,
) {
    hosted_vm.run(|script| {
        if let Some(script) = script {
            script.clear_auras();
            script.set_unit_guids(&Default::default());
            script.set_tracking(None);
        }
        cache.auras.clear();
        durations.by_slot.clear();
        // No `AuraFeedMemory` reset: its edge keys die with the VM.
    })
}

/// A `CMSG_CANCEL_AURA` per queued `CancelPlayerBuff` or `CancelTrackingBuff`, naming the spell,
/// not the slot (`0x6e7040`).
fn drain_aura_cancels(mut hosted_vm: crate::script_calls::HostedVm, net: Res<NetCommands>) {
    hosted_vm.run(|script| {
        let Some(script) = script else {
            return;
        };
        for spell_id in script.take_cancel_aura_requests() {
            let _ = net.0.send(ClientCommand::CancelAura { spell_id });
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::Reputations;

    /// [`other_unit_inputs`] as the list.
    fn other_unit_auras(store: &ObjectStore, catalog: Option<&SpellCatalog>) -> Vec<AuraState> {
        let raid = RaidFilter::default();
        let inputs: Vec<AuraInput> = other_unit_inputs(store, catalog, &raid).collect();
        aura_list(catalog, &inputs)
    }

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

    /// The feed runs in `UiFeed`, ahead of the tick, so the VM's `GetTime` is still the previous
    /// tick's, and an expiry maps from the frame step's clock before this frame's step.
    #[test]
    fn an_expiry_maps_onto_gettime_from_where_the_vm_clock_stands_before_the_tick() {
        use std::time::{Duration, Instant};
        // `UNIT_FIELD_AURA` and `UNIT_FIELD_AURAFLAGS`; slot 32's nibble is word 4's low one.
        const AURA: u16 = 47;
        const AURAFLAGS: u16 = 95;
        const ME: u64 = 0x10;
        let mut app = aura_app();
        let mut script = UiScript::new().unwrap();
        // The VM clock after the last tick.
        script.set_now(100.0);
        app.insert_non_send_resource(script);
        let fields = benilla_protocol::messages::ObjectFields::from_pairs(&[
            (AURA + 32, 11976),
            (AURAFLAGS + 4, 0x8),
        ]);
        let me = app
            .world_mut()
            .spawn((ObjectStore(fields), Guid(ME), SelfPlayer))
            .id();
        app.world_mut().resource_mut::<GuidIndex>().0.insert(ME, me);
        // Stamped at 90 s on the stamps' clock, five minutes left: it expires at 390 s.
        app.world_mut()
            .resource_mut::<AuraDurations>()
            .set(32, 300_000, 90.0);
        // This frame's step takes the clock from 100 s, where the VM stands, to 100.016 s.
        app.insert_resource(benilla_world::frame_pace::FrameStep {
            delta: Duration::from_millis(16),
            at: Instant::now(),
            elapsed: Duration::from_millis(100_016),
        });
        app.update();
        let left: f64 = app
            .world()
            .non_send_resource::<UiScript>()
            .eval("return GetPlayerBuffTimeLeft(GetPlayerBuff(0, \"HARMFUL\"))")
            .unwrap();
        assert!(
            (left - 290.0).abs() < 1.0e-9,
            "290 s left at GetTime 100 for an expiry at 390 s on the stamps' clock, not {left}"
        );
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
        restacked.level = 30;
        restacked.flags = 0x8;
        reconcile(&mut cache, &[slot(0, 10), restacked], 2.0);
        assert_eq!(order(&cache), [(0, 10), (1, 20)], "position unchanged");
        assert_eq!(cache[1].stacks, 5, "stack count refreshed");
        // The level and the flags stay as the record was made (`0x4e433b`-`0x4e434a`).
        assert_eq!(cache[1].level, 60);
        assert_eq!(cache[1].flags, AURA_FLAG_CANCELABLE | 0x8);
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

    /// The span is a signed dword that the reference adds to its clock (`0x4e4390`) and reads back
    /// as 0 once past (`0x4e4472`): a core that sends -1 for a permanent aura reads as no time
    /// left, and the slot's earlier timer is overwritten, not kept.
    #[test]
    fn a_negative_span_leaves_the_slot_reading_zero_and_drops_its_last_timer() {
        let mut durations = AuraDurations::default();
        durations.set(0, 300_000, 15.0);
        let live = join_duration(durations.by_slot.get(&0), 15.0, 15.0, 900.0);
        assert_eq!(live.0, 300.0, "control: a positive span joins");

        durations.set(0, -1, 16.0);
        assert_eq!(
            join_duration(durations.by_slot.get(&0), 15.0, 16.0, 901.0),
            (0.0, 0.0),
            "-1 is not 4,294,967 seconds, and not the earlier 300"
        );
        // The one negative the client's wrapped sign test reads as time left (`0x4e4472`).
        durations.set(1, i32::MIN, 16.0);
        let (total, _) = join_duration(durations.by_slot.get(&1), 15.0, 16.0, 901.0);
        assert_eq!(total, 2_147_483.648, "i32::MIN reads as 2^31 ms");
        durations.set(2, 0, 16.0);
        assert!(durations.by_slot.contains_key(&2), "0 ms is a stamp, now");
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

    /// `raidFilter`'s two tests over the books: a buff counts when the player or the pet knows it
    /// and it is no caster-only, flagged or area spell (`0x4b3870`); a debuff when a known spell's
    /// friendly dispel, effect 38 not aimed at an enemy, names its dispel type (`0x4b3920`).
    #[test]
    fn the_raid_filter_keeps_castable_buffs_and_dispellable_debuffs() {
        use benilla_formats::SpellDisplay;
        let row = |id: u32, f: fn(&mut SpellDisplay)| {
            let mut d = SpellDisplay {
                id,
                ..Default::default()
            };
            f(&mut d);
            (id, d)
        };
        let catalog = SpellCatalog::from_displays(
            [
                row(1243, |_| {}),
                row(1126, |_| {}),
                row(3110, |_| {}),
                row(588, |d| d.effect_implicit_target_a[0] = 1),
                row(19746, |d| d.effects[2] = 0x23),
                row(20217, |d| d.attributes_ex3 = 0x4000),
                row(527, |d| {
                    (d.effects[1], d.effect_misc_value[1]) = (38, 1);
                    d.effect_implicit_target_a[1] = 21;
                }),
                row(370, |d| {
                    (d.effects[0], d.effect_misc_value[0]) = (38, 2);
                    d.effect_implicit_target_a[0] = 6;
                }),
                row(589, |d| d.dispel = 1),
                row(980, |d| d.dispel = 2),
            ]
            .into(),
        );
        let known = [1243, 588, 19746, 20217, 527, 370];
        let raid = RaidFilter::build(known.iter(), [3110].into_iter(), Some(&catalog));
        let buff = |id| raid.keeps(Some(&catalog), 0, id);
        let debuff = |id| raid.keeps(Some(&catalog), 32, id);
        assert!(buff(1243), "known");
        assert!(!buff(1126), "unknown");
        assert!(buff(3110), "the pet's book");
        assert!(!buff(588), "on the caster alone");
        assert!(!buff(19746), "an area aura");
        assert!(!buff(20217), "AttributesEx3 0x4000");
        assert!(!buff(99999), "no row");
        assert!(
            debuff(589),
            "Magic, which the known friendly dispel removes"
        );
        assert!(
            !debuff(980),
            "Curse, whose only dispel is aimed at an enemy"
        );
        // The verdict rides on the list.
        let store = ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
            (47, 1243),
            (47 + 1, 1126),
            (95, 0x0000_0088),
        ]));
        let inputs: Vec<AuraInput> = other_unit_inputs(&store, Some(&catalog), &raid).collect();
        let list = aura_list(Some(&catalog), &inputs);
        assert_eq!(
            list.iter()
                .map(|a| (a.spell_id, a.raid_listed))
                .collect::<Vec<_>>(),
            [(1243, true), (1126, false)]
        );
    }

    /// The list holds both halves whatever the gate says: the gate rides beside it for `UnitBuff`
    /// alone, and `GameTooltip:SetUnitBuff` still reads 5916 (`0x534bfb`).
    #[test]
    fn the_list_keeps_the_helpful_half_the_unit_gate_hides() {
        let store = ridge_stalker_slots(0);
        let list = other_unit_auras(&store, None);
        assert_eq!(
            list.iter()
                .map(|a| (a.spell_id, a.helpful))
                .collect::<Vec<_>>(),
            [(5916, true), (22766, false)],
            "ascending slot, both halves"
        );
    }

    /// vmangos sets `UNIT_FLAG_AURAS_VISIBLE` for a GM viewer too (`Object.cpp:765-766`).
    #[test]
    fn detect_magic_or_gm_mode_opens_the_gate_on_the_flag_alone() {
        let reputations = Reputations::default();
        // No factions catalog: the reaction is neutral (3), below the `>= 4` bar.
        let rel = no_catalog(&reputations);
        let me = gate_viewer(0);
        let gate = |s: &ObjectStore| buffs_visible_on(&rel, &RelUnit::of(2, s), Some(&me));
        assert!(
            !gate(&ridge_stalker_slots(0)),
            "neutral reaction, not player-controlled, no PvP flag → no buffs"
        );
        assert!(
            gate(&ridge_stalker_slots(UNIT_FLAG_AURAS_VISIBLE)),
            "UNIT_FLAG_AURAS_VISIBLE alone opens it, whatever the reaction says"
        );
    }

    /// The relation inputs with no faction catalog, every faction leg neutral.
    fn no_catalog(reputations: &Reputations) -> Relations<'_> {
        Relations {
            factions: None,
            reputations,
            me: Some(1),
            roster: None,
            units: &crate::target::relations::NoUnits,
        }
    }

    /// The viewing player, guid 1, charmed by `charmer` when it is nonzero.
    fn gate_viewer(charmer: u64) -> RelUnit {
        RelUnit {
            guid: 1,
            is_player: true,
            flags: 0x8,
            charmed_by: charmer,
            ..Default::default()
        }
    }

    /// `NOT_SELECTABLE` is `CanAssist`'s; the charm clause is `UnitBuff`'s (`0x5195f1`-`0x5195f7`).
    #[test]
    fn not_selectable_and_being_charmed_each_close_the_gate_on_their_own() {
        let reputations = Reputations::default();
        let rel = no_catalog(&reputations);
        let gate =
            |s: &ObjectStore, me: &RelUnit| buffs_visible_on(&rel, &RelUnit::of(2, s), Some(me));

        // AURAS_VISIBLE is tested before `CanAssist`, so it wins over NOT_SELECTABLE.
        let both = ridge_stalker_slots(UNIT_FLAG_AURAS_VISIBLE | (1 << 25));
        assert!(
            gate(&both, &gate_viewer(0)),
            "AURAS_VISIBLE is tested FIRST and short-circuits CanAssist entirely"
        );

        // Being charmed closes the gate before `CanAssist` runs: a PvP-flagged human creature, whose
        // buffs a human sees (`IsPvP`, `0x6067bd`), is closed to a charmed one.
        let factions = crate::spell::targeting::corpse_fixture::factions();
        let rel = Relations {
            factions: Some(&factions),
            ..rel
        };
        let flagged_human = RelUnit {
            template: Some(1),
            flags: 0x1000,
            ..RelUnit::of(2, &ridge_stalker_slots(0))
        };
        let human_viewer = |charmer| RelUnit {
            template: Some(1),
            ..gate_viewer(charmer)
        };
        assert!(buffs_visible_on(
            &rel,
            &flagged_human,
            Some(&human_viewer(0))
        ));
        assert!(
            !buffs_visible_on(&rel, &flagged_human, Some(&human_viewer(0x1234))),
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
