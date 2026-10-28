//! The auras: the `UNIT_FIELD_AURA` blocks of the player and of every unit a token can name as
//! the ordered [`AuraState`] lists the aura bindings read, with the player's durations, and the
//! unit-token resolver's inputs the bindings reach them through ([`units`]). The player's buff
//! cache keeps the reference's insertion order (`0xbc6040`, repacked by `PlayerAuras_Update
//! 0x4e4170`) and is pushed where it is rebuilt; any other unit's list reads its descriptor by
//! ascending slot, or, for a group member or pet with no live object, its roster record.
//!
//! A unit's list is one derivation off its slots ([`UnitAuraSource`]), which the lent host's walk
//! reads as each getter runs ([`AuraLive`]): nothing of a unit's list is pushed.

mod units;

use std::collections::HashMap;

use bevy::prelude::*;

use benilla_formats::SpellCatalog;
use benilla_protocol::messages::{
    PartyMemberStatsInfo, UnitAuraSlot, AURA_FLAG_CANCELABLE, UNIT_AURA_POSITIVE_SLOTS,
};
use benilla_ui::script::{AuraListRead, AuraRead, AuraState, TrackingState, UiScript};

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
    /// Seconds: the full duration on an apply or refresh, only the remainder at a login (the
    /// character's load, `Player::_LoadAuras`), a relog (`Map::ExistingPlayerLogin`) or a cast
    /// pushback (`Unit::DelaySpellAuraHolder`).
    total: f64,
    /// The real-clock instant it runs out: a server-sent span counts down in real seconds.
    expires_at: f64,
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
            },
        );
    }

    /// When `slot`'s stamp runs out on the script clock, 0 for none: `script_now` is where
    /// `GetTime` stands at the instant `stamp_now` is on the stamps' clock.
    pub(crate) fn expiry(&self, slot: u8, stamp_now: f64, script_now: f64) -> f64 {
        join_duration(self.by_slot.get(&slot), stamp_now, script_now).1
    }
}

/// A record of the player's cache (`0xbc6040`), made by `BuildBuffRecord 0x4e44b0` when its aura
/// enters: the position persists, and only the stack count refreshes.
struct CachedAura {
    slot: u8,
    spell_id: u32,
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

/// [`feed_unit_guids`] seats the unit tokens the field events fan out over, which a feed that
/// reads the resolver's snapshot runs `.after`.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct UnitGuidPush;

/// Adds the resolver's feed and the session end's teardown of the aura state.
pub(crate) struct UiAuraPlugin;

impl Plugin for UiAuraPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AuraDurations>()
            .init_resource::<PlayerAuraCache>()
            .init_resource::<AuraFeedMemory>()
            .init_resource::<benilla_world::frame_pace::FrameStep>()
            // After this frame's world publish, so the gain names the unit in the same frame.
            .add_systems(
                Update,
                feed_unit_guids
                    .in_set(UnitFeed)
                    .in_set(UnitGuidPush)
                    .after(crate::ui_tooltip::MouseoverPublish),
            )
            // Torn down only at the session's end, not with the avatar, which a worldport drops.
            // After the shutdown's `PLAYER_LOGOUT` and before the glue VM goes in.
            .add_systems(
                OnExit(ClientState::InWorld),
                (end_session_aura_state, end_session_aura_stamps)
                    .in_set(crate::ui_script::SessionEnd::Clears),
            );
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
                flags: a.flags,
                level: a.level,
                stacks: a.stacks,
            });
        }
    }
}

/// What a frame repaints on; not the countdown, which the button polls.
type AuraProjection = (u32, u8, bool, Option<String>);

/// The resolver feed's memory of what each VM holds. Behind a [`crate::ui_script::VmMemo`], so a
/// `/reload`'s new VM is pushed whole.
#[derive(Resource, Default)]
struct AuraFeedMemory {
    /// What this VM was last told.
    vm: crate::ui_script::VmMemo<AuraFeedMemo>,
    /// When [`trace_timers`] last printed: host state, outliving any one VM.
    traced_at: f64,
}

/// One VM's: the player's projection as last dumped and the resolver's inputs.
#[derive(Default)]
struct AuraFeedMemo {
    /// The player's projection as last dumped, for `BENILLA_AURA_DUMP`.
    last: Vec<AuraProjection>,
    /// The resolver's inputs as last pushed.
    guids: units::UnitGuidFeed,
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
            "  slot {:>2}  spell {:>5}  {:<26}  BuffButton{button:<2}  lua {lua}",
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
        vision_hidden,
    } = input;
    let display = catalog.and_then(|cat| cat.get(spell_id));
    AuraState {
        spell_id,
        slot,
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
        until_cancelled_unknown: false,
        channeled: display.is_some_and(|d| d.attributes_ex & SPELL_ATTR_EX_IS_CHANNELED != 0),
        raid_listed,
        vision_hidden,
    }
}

/// What one aura of a unit's list is built from: its slot, spell, stack count, `AURAFLAGS` nibble,
/// `AURALEVELS` byte, `raidFilter` verdict ([`RaidFilter`]) and vision verdict ([`vision_hides`]).
/// A list is a pure function of these and the spell catalog.
#[derive(Clone, Copy, Debug, PartialEq)]
struct AuraInput {
    slot: u8,
    spell_id: u32,
    count: u8,
    flags: u8,
    level: u8,
    raid_listed: bool,
    vision_hidden: bool,
}

/// `IsAuraDisplayable`'s vision test (`0x519873`-`0x5198b8`): a spell whose `RequiredAuraVision`
/// is set shows only while the active player exists and bit `value - 1` of its
/// `PLAYER_FIELD_BYTES2` byte 1 is set, the shift's count taken mod 32 and its low byte tested
/// (`0x5198a8`-`0x5198b6`). `vision` is that byte, `None` with no active player (`0x51989a`). A
/// spell the catalog lacks keeps the display filter's deviation and shows.
fn vision_hides(catalog: Option<&SpellCatalog>, spell_id: u32, vision: Option<u8>) -> bool {
    let need = catalog
        .and_then(|c| c.get(spell_id))
        .map_or(0, |d| d.required_aura_vision);
    if need == 0 {
        return false;
    }
    let bit = (1u32 << (need.wrapping_sub(1) & 31)) as u8;
    vision.is_none_or(|v| v & bit == 0)
}

#[cfg(test)]
/// The list [`AuraInput`]s make.
fn aura_list(catalog: Option<&SpellCatalog>, inputs: &[AuraInput]) -> Vec<AuraState> {
    inputs
        .iter()
        .map(|input| unit_aura_state(catalog, input))
        .collect()
}

/// What a unit's auras are read against: the spell catalog, the `raidFilter` books
/// ([`RaidFilter`]) and the active player's `PLAYER_FIELD_BYTES2` byte 1 ([`vision_hides`]),
/// `None` with no active player.
#[derive(Clone, Copy)]
struct AuraReadCtx<'a> {
    catalog: Option<&'a SpellCatalog>,
    raid: RaidFilter<'a>,
    vision: Option<u8>,
}

/// One slot's aura of a unit's list as the walks read it, each fact derived as it is asked for:
/// its slot, spell, stack count, `AURAFLAGS` nibble and `AURALEVELS` byte, read against `ctx`. A
/// test's [`AuraInput`] is every fact of one ([`AuraInput::of`]), so the live walk and a whole
/// list are one derivation.
struct SlotAura<'a> {
    slot: u8,
    spell_id: u32,
    count: u8,
    flags: u8,
    level: u8,
    ctx: AuraReadCtx<'a>,
}

impl AuraRead for SlotAura<'_> {
    fn helpful(&self) -> bool {
        self.slot < UNIT_AURA_POSITIVE_SLOTS
    }
    fn vision_hidden(&self) -> bool {
        vision_hides(self.ctx.catalog, self.spell_id, self.ctx.vision)
    }
    fn raid_listed(&self) -> bool {
        self.ctx
            .raid
            .keeps(self.ctx.catalog, self.slot, self.spell_id)
    }
    fn state(&self) -> AuraState {
        unit_aura_state(self.ctx.catalog, &AuraInput::of(self))
    }
    fn key(&self) -> (u32, u8, u8) {
        (self.spell_id, self.level, self.count)
    }
}

impl AuraInput {
    /// Every fact `a` answers.
    fn of(a: &SlotAura) -> Self {
        AuraInput {
            slot: a.slot,
            spell_id: a.spell_id,
            count: a.count,
            flags: a.flags,
            level: a.level,
            raid_listed: a.raid_listed(),
            vision_hidden: a.vision_hidden(),
        }
    }
}

/// A unit's auras as `UnitBuff 0x519500`, `UnitDebuff 0x5198f0` and the tooltip's
/// `SetUnitBuff 0x534ac0` read them: ascending slot within each half, no durations, through the
/// display filter, every one whatever the unit gate says ([`buffs_visible_on`]), which the list
/// carries beside them.
fn store_slots<'a>(
    store: &'a ObjectStore,
    ctx: AuraReadCtx<'a>,
) -> impl Iterator<Item = SlotAura<'a>> + 'a {
    store
        .0
        .unit_auras()
        .filter(move |a| shown_in_aura_ui(ctx.catalog, a.spell_id))
        .map(move |a| SlotAura {
            slot: a.slot,
            spell_id: a.spell_id,
            count: a.stacks,
            flags: a.flags,
            level: a.level,
            ctx,
        })
}

/// A roster record's aura block as the two bindings' roster walk reads it (`0x519763`-`0x5197d2`,
/// `0x519b53`-`0x519bc5`): ascending slot, the display filter and its vision test (`0x51979f`,
/// `0x519b8f`), no unit gate and no `AURAFLAGS`, which the record does not hold. The stack count is always 1
/// (`0x51980f`, `0x519c2d`), nothing is cancelable, and the level is 0, what `SetUnitBuff` passes
/// for a record (`0x534da8`).
fn record_slots<'a>(
    slots: impl Iterator<Item = (u8, u16)> + 'a,
    ctx: AuraReadCtx<'a>,
) -> impl Iterator<Item = SlotAura<'a>> + 'a {
    slots
        .map(|(slot, spell)| (slot, u32::from(spell)))
        .filter(move |&(_, spell)| spell != 0 && shown_in_aura_ui(ctx.catalog, spell))
        .map(move |(slot, spell_id)| SlotAura {
            slot,
            spell_id,
            count: 1,
            flags: 0,
            level: 0,
            ctx,
        })
}

/// [`store_slots`] as a list's inputs, each with its `raidFilter` verdict and, against the
/// active player's `vision`, the bindings' vision verdict.
#[cfg(test)]
fn other_unit_inputs<'a>(
    store: &'a ObjectStore,
    catalog: Option<&'a SpellCatalog>,
    raid: RaidFilter<'a>,
    vision: Option<u8>,
) -> impl Iterator<Item = AuraInput> + 'a {
    let ctx = AuraReadCtx {
        catalog,
        raid,
        vision,
    };
    store_slots(store, ctx).map(|a| AuraInput::of(&a))
}

/// Where one unit's list is read from, as `UnitBuff 0x519500` finds it: the object its token
/// resolves (`0x51955e`), else a roster record (`0x519595` → `0x519735`). The lent host's walk
/// reads it as the call runs ([`AuraLive`]); a test reads it whole ([`UnitAuraSource::list`]).
pub(crate) struct UnitAuraSource<'a> {
    from: AuraFrom<'a>,
    ctx: AuraReadCtx<'a>,
}

enum AuraFrom<'a> {
    /// The live unit's slots, and `UnitBuff`'s unit gate on it.
    Object {
        guid: u64,
        store: &'a ObjectStore,
        gate: &'a dyn Fn(u64, &ObjectStore) -> bool,
    },
    /// A roster record's block in `records`, which has no gate (`0x519735`); none without a
    /// roster.
    Record {
        guid: u64,
        records: Option<&'a HashMap<u64, PartyMemberStatsInfo>>,
    },
}

impl<'a> UnitAuraSource<'a> {
    /// `guid`'s list: off `store`, its object, gated through `gate`; else, when its roster record
    /// may stand in (`record_ok`), off its record in `records`; `None` for neither, as
    /// `UnitBuff`'s roster walk finds nothing for a guid with no record.
    fn of(
        guid: u64,
        store: Option<&'a ObjectStore>,
        record_ok: bool,
        records: Option<&'a HashMap<u64, PartyMemberStatsInfo>>,
        gate: &'a dyn Fn(u64, &ObjectStore) -> bool,
        ctx: AuraReadCtx<'a>,
    ) -> Option<Self> {
        let from = match store {
            Some(store) => AuraFrom::Object { guid, store, gate },
            None if record_ok => AuraFrom::Record { guid, records },
            None => return None,
        };
        Some(Self { from, ctx })
    }

    /// Each aura in ascending slot, until `visit` answers `true`.
    fn each(&self, mut visit: impl FnMut(&SlotAura<'a>) -> bool) {
        match self.from {
            AuraFrom::Object { store, .. } => {
                for a in store_slots(store, self.ctx) {
                    if visit(&a) {
                        return;
                    }
                }
            }
            AuraFrom::Record {
                guid,
                records: Some(records),
            } => {
                let slots = crate::ui_party::roster_aura_slots_in(records, guid);
                for a in record_slots(slots, self.ctx) {
                    if visit(&a) {
                        return;
                    }
                }
            }
            AuraFrom::Record { records: None, .. } => {}
        }
    }

    /// `UnitBuff`'s unit gate: the object's ([`buffs_visible_on`]); a record's is open.
    fn gate(&self) -> bool {
        match self.from {
            AuraFrom::Object { guid, store, gate } => gate(guid, store),
            AuraFrom::Record { .. } => true,
        }
    }

    #[cfg(test)]
    /// Every aura's inputs, into `inputs`, with the unit gate.
    fn inputs(&self, inputs: &mut Vec<AuraInput>) -> bool {
        self.each(|a| {
            inputs.push(AuraInput::of(a));
            false
        });
        self.gate()
    }

    #[cfg(test)]
    /// The list whole, as a feed pushes it.
    fn list(&self) -> benilla_ui::script::UnitAuraList {
        let mut inputs = Vec::new();
        let buffs_visible = self.inputs(&mut inputs);
        benilla_ui::script::UnitAuraList {
            auras: aura_list(self.ctx.catalog, &inputs),
            buffs_visible,
        }
    }
}

impl AuraListRead for UnitAuraSource<'_> {
    fn buffs_visible(&self) -> bool {
        self.gate()
    }
    fn find(&self, visit: &mut dyn FnMut(&dyn AuraRead) -> bool) {
        self.each(|a| visit(a));
    }
}

/// What a set `raidFilter` keeps, the same test on the live walk and the record's (`UnitBuff`
/// `0x51967b`, `0x5197a8`; `UnitDebuff` `0x519a1a`; the tooltip's `0x534c6c`, `0x534d6b`,
/// `0x534fec`): a buff the player could cast (`0x4b3870`), a debuff the player can dispel
/// (`0x4b3920`). Read off the player's book and the pet's as they stand at the call.
#[derive(Clone, Copy, Default)]
struct RaidFilter<'a> {
    /// The player's known spells, the bits at `[0xb710fc]` that every learn sets (`0x4b2603`), and
    /// the dispel list `0xb71118` the learns append ([`crate::ui_action::PlayerActions::dispels`]).
    book: Option<&'a crate::ui_action::PlayerActions>,
    /// The pet bar, whose book `0xb6f098` `0x4b3870` scans when the player does not know the spell
    /// (`0x4b38a0`).
    pet: Option<&'a crate::ui_pet::PetBar>,
}

impl RaidFilter<'_> {
    /// Whether `spell_id` is in the pet's book: a bar word's spell that passes the book's add gate
    /// (`0x4b2f90`, `ui_pet_book`), while the bar is up.
    fn in_pet_book(&self, catalog: Option<&SpellCatalog>, spell_id: u32) -> bool {
        self.pet.filter(|pet| pet.has_bar()).is_some_and(|pet| {
            pet.spells.spells.iter().any(|e| {
                e.action() == spell_id
                    && catalog
                        .and_then(|c| c.get(spell_id))
                        .is_some_and(|d| d.in_pet_book())
            })
        })
    }

    /// The verdict on the aura in `slot`: the buff test on the first 32, the debuff test after.
    fn keeps(&self, catalog: Option<&SpellCatalog>, slot: u8, spell_id: u32) -> bool {
        let Some(d) = catalog.and_then(|c| c.get(spell_id)) else {
            // Both tests read the row, which the walks require first (`0x519669`, `0x4b38ca`).
            return false;
        };
        if slot >= UNIT_AURA_POSITIVE_SLOTS {
            // `0x4b3920(Dispel)`, the row's `+0x10` (`0x519a1c`).
            return self.book.is_some_and(|b| b.dispels.contains(&d.dispel));
        }
        // `0x4b3870`: known or in the pet's book (`0x4b3893`, `0x4b38a0`), then `AttributesEx3`
        // bit `0x4000` clear (`0x4b38cf`), the first effect not aimed at the caster
        // (`ImplicitTargetA` 1, `0x4b38d4`), and no effect of 35, 119, 128, 129 or 27, the party,
        // pet and area auras and the persistent area aura (`0x4b38e5`-`0x4b3902`).
        (self.book.is_some_and(|b| b.spells.contains(&spell_id))
            || self.in_pet_book(catalog, spell_id))
            && d.attributes_ex3 & 0x4000 == 0
            && d.effect_implicit_target_a[0] != TARGET_SELF
            && !d
                .effects
                .iter()
                .any(|e| matches!(e, 0x23 | 0x77 | 0x80 | 0x81 | 0x1b))
    }
}

/// `TARGET_UNIT_CASTER` (vmangos `SpellDefines.h:26`), the first effect's target that fails the
/// buff test (`0x4b38d4`).
const TARGET_SELF: u32 = 1;

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
/// expiry, because a stale value is no mechanism to reproduce. `None` is that miss: the call reads
/// the expiry ([`AuraState::until_cancelled_unknown`]).
fn until_cancelled(
    display: Option<&benilla_formats::SpellDisplay>,
    duration_row: Option<&benilla_formats::SpellDuration>,
    cancelable: bool,
) -> Option<bool> {
    let d = display?;
    let permanent = match duration_row {
        Some(row) => row.base_ms < 0 && row.per_level_ms <= 0,
        // A negative, out-of-range or empty `DurationIndex`: all three reach `0x4e4580`.
        None => true,
    };
    Some(
        permanent
            || (!cancelable
                && d.effects
                    .iter()
                    .any(|e| matches!(e, 0x23 | 0x77 | 0x80 | 0x81))),
    )
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
struct GuidSources<'w> {
    selection: Res<'w, Selection>,
    // `Option`: a UI-only harness lacks their plugins; without the party's no group token names
    // anyone.
    group: Option<Res<'w, crate::ui_party::GroupState>>,
    interact: Option<Res<'w, crate::ui_session::InteractNpc>>,
}

/// The unit-token resolver's inputs ([`benilla_ui::script::UnitGuids`]) as the VM holds them, the
/// snapshot a getter reads with no host to ask and the app's readers of the held units walk. The
/// aura getters read the units' slots at the call through the host ([`AuraLive`]), and the
/// player's buff cache is pushed where it is rebuilt: at the world entry ([`enter_world`]) and in
/// each aura watcher's run ([`PlayerAuraWatch::run`]).
fn feed_unit_guids(
    mut vm: crate::script_calls::VmWith<GuidSources<'static>>,
    self_q: Query<(&ObjectStore, &Guid), With<SelfPlayer>>,
    stores: Query<&ObjectStore>,
    index: Res<GuidIndex>,
    mut mem: ResMut<AuraFeedMemory>,
    cache: Res<PlayerAuraCache>,
    spells: Option<Res<Spells>>,
    time: Res<Time<Real>>,
) {
    vm.turn(|script, units| {
        let Some(script) = script else {
            return;
        };
        let mem = &mut *mem;
        let Ok((store, self_guid)) = self_q.single() else {
            // No avatar keeps the last inputs: a worldport re-streams the avatar mid-session.
            return;
        };
        let bevy_now = time.elapsed_secs_f64();
        let catalog = spells.as_ref().map(|s| &s.catalog);
        if let Some(period) = trace_period() {
            if bevy_now - mem.traced_at >= period {
                mem.traced_at = bevy_now;
                let list =
                    player_list(&cache.auras, catalog, spells.as_ref().map(|s| &s.durations));
                trace_timers(script, &cache.auras, &list, bevy_now);
            }
        }
        let memo = mem.vm.get(script);
        if std::env::var_os("BENILLA_AURA_DUMP").is_some() {
            let list = player_list(&cache.auras, catalog, spells.as_ref().map(|s| &s.durations));
            let projection = projection_of(&list);
            if projection != memo.last {
                dump_cache(&cache.auras, catalog);
            }
            memo.last = projection;
        }
        let held = |guid: u64| index.0.get(&guid).and_then(|&e| stores.get(e).ok());
        // The resolver reads the published pair `[0xb4e2c8]` (`0x515bb6`, `0x515bdb`), not the
        // pick, and takes it only as a unit (`0x515bca mov ecx,8`, `0x515bd9 je`): a GameObject, a
        // corpse or nothing names nobody.
        let mouseover = script
            .world_mouseover()
            .filter(|&guid| held(guid).is_some_and(|s| s.is_unit()))
            .unwrap_or(0);
        memo.guids.feed(
            script,
            units::Inputs {
                group: units.group.as_deref(),
                me: self_guid.0,
                pet: crate::ui_unit::pet_token_guid(Some(store)),
                target: units.selection.guid.unwrap_or(0),
                mouseover,
                npc: units.interact.as_deref().and_then(|i| i.1).unwrap_or(0),
                held,
            },
        );
    })
}

/// `BENILLA_AURA_DUMP`'s print of the player's cache, on a change.
fn dump_cache(cache: &[CachedAura], catalog: Option<&SpellCatalog>) {
    info!("aura dump: {} aura(s) on the player bar", cache.len());
    for c in cache {
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

/// What the world entry's setup of the buff cache reads and writes ([`enter_world`]).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct AuraEntry<'w, 's> {
    // `Option`: a UI-only harness has no aura plugin, and so no cache to set up.
    cache: Option<ResMut<'w, PlayerAuraCache>>,
    spells: Option<Res<'w, Spells>>,
    time: Option<Res<'w, Time<Real>>>,
    me: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
}

/// The world entry's setup of the buff cache, inside the cascade (`0x490983` → `0x4e4100`): the
/// watcher registered on the new player object, then one rebuild and its `PLAYER_AURAS_CHANGED`
/// (`0x4e412e`, `0x4e437f`). A first create runs no watcher (`0x465d80`), so this is how the cache
/// meets a new player object's auras: at a login, a worldport and a `/reload`'s cascade. Past it,
/// only the watcher rebuilds, once per packet that moves the aura fields
/// ([`PlayerAuraWatch::run`]). With no active player object the rebuild returns before its signal
/// (`0x4e4196`).
pub(crate) fn enter_world(script: &mut UiScript, entry: &mut AuraEntry) {
    let Some(cache) = entry.cache.as_deref_mut() else {
        return;
    };
    let Ok(store) = entry.me.single() else {
        return;
    };
    let catalog = entry.spells.as_ref().map(|s| &s.catalog);
    let now = entry.time.as_ref().map_or(0.0, |t| t.elapsed_secs_f64());
    let tracking = rebuild(&mut cache.auras, store, catalog, now);
    script.set_tracking(tracking);
    script.set_player_auras(player_list(
        &cache.auras,
        catalog,
        entry.spells.as_ref().map(|s| &s.durations),
    ));
    // The cache rebuild's own event, no arguments (`0x4e437f`): the stock buff bar and
    // `MiniMapTrackingFrame` register it.
    script.fire_event("PLAYER_AURAS_CHANGED", vec![]);
}

/// [`enter_world`] in the cascade's place ([`crate::ui_world_enter`]), with the host lent.
pub(crate) fn enter_world_in_place(mut vm: crate::script_calls::HostedVm, mut entry: AuraEntry) {
    vm.run(|script| {
        if let Some(script) = script {
            enter_world(script, &mut entry);
        }
    })
}

/// `PlayerAuras_Update` (`0x4e4170`) over the player's descriptor `store`: the cache's two passes
/// ([`reconcile`]) over the slots the attribute test keeps (`0x4e42b6`-`0x4e42c8`), and the
/// tracking global the walk leaves (`0x4e4302`).
fn rebuild(
    cache: &mut Vec<CachedAura>,
    store: &ObjectStore,
    catalog: Option<&SpellCatalog>,
    now: f64,
) -> Option<TrackingState> {
    let occupied: Vec<UnitAuraSlot> = store.0.unit_auras().collect();
    // Filtered before the cache, so a hidden aura takes no position (`0x4e42b6`-`0x4e4308`).
    let live: Vec<UnitAuraSlot> = occupied
        .iter()
        .filter(|a| shown_in_aura_ui(catalog, a.spell_id))
        .copied()
        .collect();
    reconcile(cache, &live, now);
    tracking_state_of(catalog, &occupied)
}

/// The cache as `GetPlayerBuff` and its family read it. It carries no duration: the timers read
/// the expiry array through the lent host ([`AuraLive`]), as the reference's reader does at the
/// call (`0x4e4450`).
fn player_list(
    cache: &[CachedAura],
    catalog: Option<&SpellCatalog>,
    spell_durations: Option<&benilla_formats::SpellDurationCatalog>,
) -> Vec<AuraState> {
    cache
        .iter()
        .map(|c| {
            let display = catalog.and_then(|cat| cat.get(c.spell_id));
            let flag = until_cancelled(
                display,
                display.and_then(|d| spell_durations.and_then(|c| c.get(d.duration_index))),
                c.flags & AURA_FLAG_CANCELABLE != 0,
            );
            AuraState {
                spell_id: c.spell_id,
                slot: c.slot,
                name: display.map(|d| d.name.clone()),
                icon: display.and_then(|d| d.icon.clone()),
                count: c.stacks,
                level: c.level,
                debuff_type: display
                    .zip(catalog)
                    .and_then(|(d, cat)| cat.dispel_name(d))
                    .map(str::to_string),
                duration: 0.0,
                expiration_time: 0.0,
                helpful: c.slot < UNIT_AURA_POSITIVE_SLOTS,
                cancelable: c.flags & AURA_FLAG_CANCELABLE != 0,
                until_cancelled: flag.unwrap_or(false),
                until_cancelled_unknown: flag.is_none(),
                channeled: display
                    .is_some_and(|d| d.attributes_ex & SPELL_ATTR_EX_IS_CHANNELED != 0),
                // No reader of the buff cache takes a `raidFilter`, nor runs the vision test
                // (`0x4e4170`).
                raid_listed: false,
                vision_hidden: false,
            }
        })
        .collect()
}

/// `OnPlayerAurasChanged` (`0x4e4140`), the active player's watcher on the aura fields, 0x138
/// bytes from `UNIT_FIELD_AURA` (`0x4e4109`, `0x4e4115`), registered at the world entry
/// (`0x4e4100`): the cache's rebuild off the descriptor as the watcher's packet left it, then
/// `PLAYER_AURAS_CHANGED` (`0x4e437a`, `0x4e437f`), whatever the rebuild changed. With no active
/// player object the rebuild returns before its signal (`0x4e4196`).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct PlayerAuraWatch<'w, 's> {
    // `Option`: a UI-only harness has no aura plugin, and so no cache to rebuild.
    cache: Option<ResMut<'w, PlayerAuraCache>>,
    spells: Option<Res<'w, Spells>>,
    time: Option<Res<'w, Time<Real>>>,
    stores: crate::ui_unit::views::ViewStores<'w, 's>,
    me: Query<'w, 's, Entity, With<SelfPlayer>>,
}

impl PlayerAuraWatch<'_, '_> {
    /// The watcher's run.
    pub(crate) fn run(&mut self, script: &mut UiScript) {
        let Some(cache) = self.cache.as_deref_mut() else {
            return;
        };
        let Some(store) = self.me.iter().next().and_then(|e| self.stores.get(e).ok()) else {
            return;
        };
        let catalog = self.spells.as_ref().map(|s| &s.catalog);
        let now = self.time.as_ref().map_or(0.0, |t| t.elapsed_secs_f64());
        let tracking = rebuild(&mut cache.auras, store, catalog, now);
        let list = player_list(
            &cache.auras,
            catalog,
            self.spells.as_ref().map(|s| &s.durations),
        );
        script.set_player_auras(list);
        script.set_tracking(tracking);
        script.fire_event("PLAYER_AURAS_CHANGED", vec![]);
    }
}

/// What the views lane reads to build a unit's aura list as a packet left it
/// ([`crate::ui_unit::views`]), and the lent host's walk reads it ([`AuraLive`]): the spell
/// catalog and the two books `raidFilter` tests against ([`RaidFilter`]).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct AuraViews<'w> {
    spells: Option<Res<'w, Spells>>,
    // `Option`: a UI-only harness has no action store or pet bar, and so no book to test against.
    book: Option<Res<'w, crate::ui_action::PlayerActions>>,
    pet: Option<Res<'w, crate::ui_pet::PetBar>>,
}

impl AuraViews<'_> {
    /// `f` over `guid`'s [`UnitAuraSource`]: off `store`, its descriptor, else, when its roster
    /// record may stand in (`record_ok`), off its record in `records`; `None` for neither. `me`
    /// is our own descriptor as of the same place, whose vision the list is read against and
    /// whose standing `UnitBuff`'s gate reads (`0x5195be`-`0x5195f7`).
    #[allow(clippy::too_many_arguments)] // the list's inputs, each as of the place it is read for
    pub(crate) fn read<T>(
        &self,
        guid: u64,
        store: Option<&ObjectStore>,
        record_ok: bool,
        records: Option<&HashMap<u64, PartyMemberStatsInfo>>,
        relations: &crate::target::RelationWorld,
        me: Option<&ObjectStore>,
        f: impl FnOnce(Option<&UnitAuraSource>) -> T,
    ) -> T {
        let ctx = AuraReadCtx {
            catalog: self.spells.as_ref().map(|s| &s.catalog),
            raid: RaidFilter {
                book: self.book.as_deref(),
                pet: self.pet.as_deref(),
            },
            vision: me.map(|s| s.0.player_aura_vision()),
        };
        let rel = relations.get();
        let viewer = relations
            .me_guid()
            .zip(me)
            .map(|(g, s)| relations.unit_of(g, s));
        let gate = |guid: u64, unit: &ObjectStore| {
            buffs_visible_on(&rel, &relations.unit_of(guid, unit), viewer.as_ref())
        };
        let source = UnitAuraSource::of(guid, store, record_ok, records, &gate, ctx);
        f(source.as_ref())
    }

    #[cfg(test)]
    /// `guid`'s list as [`Self::read`] reads it, whole, a group token's off the roster in
    /// `group`.
    pub(crate) fn list(
        &self,
        guid: u64,
        store: Option<&ObjectStore>,
        record_ok: bool,
        group: Option<&crate::ui_party::GroupState>,
        relations: &crate::target::RelationWorld,
        me: Option<&ObjectStore>,
    ) -> Option<benilla_ui::script::UnitAuraList> {
        let records = group.map(|g| &g.stats);
        self.read(guid, store, record_ok, records, relations, me, |source| {
            source.map(UnitAuraSource::list)
        })
    }
}

/// The aura slots the unit aura getters read off as they run, as the reference's walk the object
/// their token resolves (`UnitBuff` `0x519500`, `0x51955e`): each store and record as of the
/// drain's place, so a listener reads its own packet's, and the active player as of there, whose
/// vision and standing the walk reads.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct AuraLive<'w, 's> {
    objects: crate::net::Objects<'w, 's>,
    stores: crate::ui_unit::views::ViewStores<'w, 's>,
    views: AuraViews<'w>,
    relations: crate::target::RelationWorld<'w, 's>,
    // `Option`: a harness without the party's has no roster, and so no record to read.
    group: Option<Res<'w, crate::ui_party::GroupState>>,
    /// The player's duration stamps, read as they stand.
    durations: Option<Res<'w, AuraDurations>>,
    /// The interface clock's pair and the paced clock the stamps' seconds run on, which map a
    /// stamp onto `GetTime`.
    clock: Option<Res<'w, crate::ui_script::HostClock>>,
    step: Option<Res<'w, benilla_world::frame_pace::FrameStep>>,
}

/// The expiry array's read at the call (`0x4e4450`): the slot's stamp on the script clock, mapped
/// through the interface clock's pair ([`crate::ui_script::HostClock`]): its `GetTime` and the
/// stamps' clock at its anchor; declined with no clock.
impl crate::script_calls::live::LiveFamily for AuraLive<'_, '_> {
    fn answer(&mut self, ask: &mut benilla_ui::script::live::LiveAsk<'_>) -> bool {
        ask.answer(|q: &benilla_ui::script::PlayerBuffExpiry| {
            let (clock, step) = (&self.clock.as_deref()?.0, self.step.as_deref()?);
            Some(self.durations.as_deref()?.expiry(
                q.slot,
                stamp_clock_at(step, clock.anchor),
                clock.ui_now,
            ))
        })
    }
}

/// The stamps' clock, in seconds, at `at`: the paced clock steps with the wall clock it stands
/// at ([`benilla_world::frame_pace::FrameStep`]), so its reading at an earlier step's instant is
/// its reading now less the time between.
fn stamp_clock_at(step: &benilla_world::frame_pace::FrameStep, at: std::time::Instant) -> f64 {
    let now = step.elapsed_secs_f64();
    match step.at.checked_duration_since(at) {
        Some(behind) => now - behind.as_secs_f64(),
        None => now + at.duration_since(step.at).as_secs_f64(),
    }
}

impl AuraLive<'_, '_> {
    /// Hand `read` `guid`'s aura list as of here: its object's slots, else its roster record's,
    /// which `0x496400` and `0x496420` look up by guid whatever token named it (`0x519735`);
    /// `None` for neither. Whether it read, as it always does.
    pub(crate) fn read(&self, guid: u64, read: &mut dyn FnMut(Option<&dyn AuraListRead>)) -> bool {
        let me = self.objects.player().and_then(|e| self.stores.get(e).ok());
        let store = self.stores.by_guid(guid, self.objects.store(guid));
        let records = self.group.as_deref().map(|g| self.stores.records(&g.stats));
        self.views
            .read(guid, store, true, records, &self.relations, me, |source| {
                read(source.map(|s| s as &dyn AuraListRead))
            });
        true
    }
}

/// [`feed_unit_guids`] once, for a test that drives the feeds by hand.
#[cfg(test)]
pub(crate) fn run_feed_unit_guids_once(world: &mut World) {
    world
        .run_system_cached(feed_unit_guids)
        .expect("the resolver's feed runs as a one-shot system");
}

/// Joins a slot's stamp to the aura in that slot as `(duration, expirationTime)` on the script
/// clock, `(0, 0)` for none. The reference's reader (`0x4e4450`) returns `expiry - now` for the
/// slot with no freshness test, so a stamp from before its aura joins it: a login's durations go
/// out while the server loads the character, seconds ahead of the auras. `stamp_now` is the
/// stamps' clock at the instant `script_now` stands at, so the expiry holds still frame to frame.
fn join_duration(stamp: Option<&DurationStamp>, stamp_now: f64, script_now: f64) -> (f64, f64) {
    stamp
        .map(|d| (d.total, script_now + (d.expires_at - stamp_now)))
        .unwrap_or((0.0, 0.0))
}

/// The session's end: drops the lists and stamps, so nothing reaches the next character's bar.
/// Deviation: the reference clears its expiry array (`0xbc5f68`) at every game entry, in
/// `ResetBuffCache` (`0x4e40c0`, called from `0x48f5a9`); benilla clears it at exit instead.
fn end_session_aura_state(
    mut hosted_vm: crate::script_calls::HostedVm,
    mut cache: ResMut<PlayerAuraCache>,
) {
    hosted_vm.run(|script| {
        if let Some(script) = script {
            script.clear_auras();
            script.set_unit_guids(&Default::default());
            script.set_tracking(None);
        }
        cache.auras.clear();
        // No `AuraFeedMemory` reset: its edge keys die with the VM.
    })
}

/// The stamps' half of [`end_session_aura_state`], in a system of its own: the system that lends
/// the host cannot write what the host's reads read ([`AuraLive`]).
fn end_session_aura_stamps(mut durations: ResMut<AuraDurations>) {
    durations.by_slot.clear();
}

/// The aura cancel `0x6e7040` (`CancelPlayerBuff`, `CancelTrackingBuff`, `CancelAreaSpiritHeal`
/// inside their calls, and the healer scan's wave cancel): the active player's halves, the
/// far-sight gate, then the packet, which names the spell, not the slot. The host signals the area
/// spirit healer aura's event between the gate and the packet
/// ([`benilla_ui::script::AuraCall`]).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct AuraSteps<'w, 's> {
    me: Query<'w, 's, (Entity, &'static ObjectStore), With<SelfPlayer>>,
    index: Option<Res<'w, GuidIndex>>,
    gate: AuraGate<'w>,
}

/// What the cancel `0x6e7040` reads beside the active player and the object index, which its
/// caller lends: a system that already holds the stores takes this alone ([`AuraSteps`] is this
/// with the reads it brings itself).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct AuraGate<'w> {
    /// Absent in a harness without the unit changes' queue.
    edits: Option<Res<'w, crate::creature_anim::UnitEdits>>,
    /// The record the gate reads; absent without client data, which gates nothing.
    spells: Option<Res<'w, Spells>>,
    net: Res<'w, NetCommands>,
    /// The machine `UpdateFarSight` runs; absent in a harness without the player's plugin.
    far_sight: Option<Res<'w, crate::player::FarSight>>,
}

/// `AttributesEx` 0x2000, the far-sight spells, which the cancel hands to `UpdateFarSight`.
const ATTR_EX_FAR_SIGHT: u32 = 0x2000;
/// `AttributesEx` 0x4, the channelled spells, which the cancel's far-sight gate passes.
const ATTR_EX_CHANNELED: u32 = 0x4;

/// The active player as the cancel reads it: its entity and `PLAYER_FARSIGHT`.
pub(crate) type CancelPlayer = (Entity, Option<u64>);

impl AuraGate<'_> {
    /// For an active player: the precast of `spell_id` ended with its tail
    /// (`0x60d040(player, spell, 1, 1)` at `0x6e7075`), then the spell's kits stopped
    /// (`0x614150(player, spell, 0)` at `0x6e707e`). Both are unit edits, applied at the pass's
    /// end with the rest of the frame's: the components they write are read beside the call host.
    pub(crate) fn end_on_player(&self, player: Option<CancelPlayer>, spell_id: u32) {
        let (Some((entity, _)), Some(edits)) = (player, self.edits.as_deref()) else {
            return;
        };
        edits.push(crate::creature_anim::UnitEdit::EndPrecast {
            entity,
            spell_id: Some(spell_id),
            tail: true,
        });
        edits.push(crate::creature_anim::UnitEdit::StopSpellKits { entity, spell_id });
    }

    /// The far-sight gate (`0x6e7083`-`0x6e70b4`): a spell with `AttributesEx` 0x2000 and without
    /// 0x4 stops the cancel with no player, or else runs `UpdateFarSight` (`0x5ee290`, at
    /// `0x6e70ad`, [`crate::player::FarSight::update`]), which toggles the view and tells the
    /// server, and stops the cancel when it answers nonzero: unless `PLAYER_FARSIGHT` names an
    /// object the client does not hold (`0x5ee32b` → `0x5ee360`), `held` saying which it holds.
    /// Whether the cancel goes on to its event and packet.
    pub(crate) fn passes_far_sight(
        &self,
        player: Option<CancelPlayer>,
        held: impl Fn(u64) -> bool,
        spell_id: u32,
    ) -> bool {
        let ex = self
            .spells
            .as_deref()
            .and_then(|s| s.catalog.get(spell_id))
            .map_or(0, |d| d.attributes_ex);
        if ex & ATTR_EX_FAR_SIGHT == 0 || ex & ATTR_EX_CHANNELED != 0 {
            return true;
        }
        let Some((_, field)) = player else {
            return false;
        };
        match self.far_sight.as_deref() {
            Some(far_sight) => !far_sight.update(field, held, &self.net),
            // No machine: its answer alone.
            None => field.is_some_and(|guid| !held(guid)),
        }
    }

    /// `CMSG_CANCEL_AURA` (`0x6e70eb`), the spell's id.
    pub(crate) fn send(&self, spell_id: u32) {
        let _ = self.net.0.send(ClientCommand::CancelAura { spell_id });
    }

    /// `0x6e7040(spell)` from a packet handler, the time packet's scan (`0x492296` → `0x492201`)
    /// and the ghost flag's clear (`0x5dea20`), whose signals go on the notice lane at the
    /// packet's place: the halves, the gate and the packet here, the event left to the lane.
    /// Whether the area spirit healer aura's event is owed.
    pub(crate) fn cancel_at_packet(
        &self,
        player: Option<CancelPlayer>,
        held: impl Fn(u64) -> bool,
        spell_id: u32,
    ) -> bool {
        self.end_on_player(player, spell_id);
        if !self.passes_far_sight(player, held, spell_id) {
            return false;
        }
        self.send(spell_id);
        spell_id == crate::ui_dialog_verbs::AREA_SPIRIT_HEALER_AURA
    }
}

impl AuraSteps<'_, '_> {
    fn player(&self) -> Option<CancelPlayer> {
        self.me
            .single()
            .ok()
            .map(|(entity, store)| (entity, store.0.player_farsight()))
    }

    fn held(&self) -> impl Fn(u64) -> bool + '_ {
        |guid| {
            self.index
                .as_deref()
                .is_some_and(|index| index.0.contains_key(&guid))
        }
    }

    /// [`AuraGate::end_on_player`] for the active player.
    pub(crate) fn end_on_player(&self, spell_id: u32) {
        self.gate.end_on_player(self.player(), spell_id);
    }

    /// [`AuraGate::passes_far_sight`] for the active player.
    pub(crate) fn passes_far_sight(&self, spell_id: u32) -> bool {
        self.gate
            .passes_far_sight(self.player(), self.held(), spell_id)
    }

    /// `CMSG_CANCEL_AURA` (`0x6e70eb`), the spell's id.
    pub(crate) fn send(&self, spell_id: u32) {
        self.gate.send(spell_id);
    }

    /// `0x6e7040(spell)` outside a script call, the healer scan's (`0x492201`): its event fired
    /// into `script`, if given.
    pub(crate) fn cancel(&self, script: Option<&mut UiScript>, spell_id: u32) {
        self.end_on_player(spell_id);
        if !self.passes_far_sight(spell_id) {
            return;
        }
        if spell_id == crate::ui_dialog_verbs::AREA_SPIRIT_HEALER_AURA {
            if let Some(script) = script {
                script.fire_event("AREA_SPIRIT_HEALER_OUT_OF_RANGE", vec![]);
            }
        }
        self.send(spell_id);
    }

    /// [`AuraGate::cancel_at_packet`] for the active player.
    pub(crate) fn cancel_at_packet(&self, spell_id: u32) -> bool {
        self.gate
            .cancel_at_packet(self.player(), self.held(), spell_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::Reputations;

    /// [`other_unit_inputs`] as the list.
    fn other_unit_auras(store: &ObjectStore, catalog: Option<&SpellCatalog>) -> Vec<AuraState> {
        let inputs: Vec<AuraInput> =
            other_unit_inputs(store, catalog, RaidFilter::default(), None).collect();
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
                flags: 0x8,
                level: 60,
                stacks: 1,
            });
    }

    /// The lent host maps a stamp onto `GetTime` through the interface clock's pair: its `GetTime`
    /// and the stamps' clock at its anchor. Ahead of this frame's tick the pair is the last tick's,
    /// at the step's start; after it, this one's: the expiry is the same either way.
    #[test]
    fn an_expiry_maps_onto_gettime_through_the_interface_clocks_pair() {
        use std::time::{Duration, Instant};
        let mut durations = AuraDurations::default();
        // Stamped at 90 s on the stamps' clock, five minutes left: it expires at 390 s.
        durations.set(32, 300_000, 90.0);
        // This frame's step takes the clock from 100 s to 100.016 s.
        let at = Instant::now();
        let step = benilla_world::frame_pace::FrameStep {
            delta: Duration::from_millis(16),
            at,
            elapsed: Duration::from_millis(100_016),
        };
        // The VM's clock stands where the stamps' does, as the VM's seed puts it.
        let expiry = |anchor: Instant, ui_now: f64| {
            durations.expiry(32, stamp_clock_at(&step, anchor), ui_now)
        };
        let before = expiry(at - Duration::from_millis(16), 100.0);
        let after = expiry(at, 100.016);
        assert!((before - 390.0).abs() < 1.0e-9, "{before}");
        assert!((after - 390.0).abs() < 1.0e-9, "{after}");
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
            "the display cache survives the gap"
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
    }

    #[test]
    fn a_duration_is_rebased_onto_the_script_clock() {
        let stamp = DurationStamp {
            total: 30.0,
            expires_at: 230.0,
        };
        assert_eq!(join_duration(Some(&stamp), 200.0, 5000.0), (30.0, 5030.0));
        // No stamp: a permanent aura, which gets no packet.
        assert_eq!(join_duration(None, 200.0, 5000.0), (0.0, 0.0));
    }

    /// The span is a signed dword that the reference adds to its clock (`0x4e4390`) and reads back
    /// as 0 once past (`0x4e4472`): a core that sends -1 for a permanent aura reads as no time
    /// left, and the slot's earlier timer is overwritten, not kept.
    #[test]
    fn a_negative_span_leaves_the_slot_reading_zero_and_drops_its_last_timer() {
        let mut durations = AuraDurations::default();
        durations.set(0, 300_000, 15.0);
        let live = join_duration(durations.by_slot.get(&0), 15.0, 900.0);
        assert_eq!(live.0, 300.0, "control: a positive span joins");

        durations.set(0, -1, 16.0);
        assert_eq!(
            join_duration(durations.by_slot.get(&0), 16.0, 901.0),
            (0.0, 0.0),
            "-1 is not 4,294,967 seconds, and not the earlier 300"
        );
        // The one negative the client's wrapped sign test reads as time left (`0x4e4472`).
        durations.set(1, i32::MIN, 16.0);
        let (total, _) = join_duration(durations.by_slot.get(&1), 16.0, 901.0);
        assert_eq!(total, 2_147_483.648, "i32::MIN reads as 2^31 ms");
        durations.set(2, 0, 16.0);
        assert!(durations.by_slot.contains_key(&2), "0 ms is a stamp, now");
    }

    /// A login's durations go out while the server loads the character (vmangos `_LoadAuras`,
    /// `Player.cpp:15579`, through `SpellAuraHolder::UpdateAuraDuration`), seconds before the
    /// in-game UI reads the auras. The reference stores `now + duration` (`0x4e4390`) and its
    /// reader returns `expiry - now` (`0x4e4450`) with no freshness test, so the stamp still joins.
    #[test]
    fn a_stamp_that_arrives_seconds_before_its_aura_still_joins_it() {
        let mut durations = AuraDurations::default();
        // The live measurement: the stamp at 15.49, the aura at 36.44.
        durations.set(0, 300_000, 15.49);
        let mut cache = Vec::new();
        reconcile(&mut cache, &[slot(0, 1126)], 36.44);
        let list = player_list(&cache, None, None);
        assert_eq!(
            list[0].slot, 0,
            "the record names the slot the stamp is read by"
        );
        // 300 s from 15.49 leave 279.05 at 36.44: script clock 900 plus that.
        let expiry = durations.expiry(list[0].slot, 36.44, 900.0);
        assert!(
            (expiry - 1179.05).abs() < 1e-9,
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

    /// `raidFilter`'s two tests over the books as they stand: a buff counts when the player or the
    /// pet knows it and it is no caster-only, flagged or area spell (`0x4b3870`); a debuff when the
    /// dispel list the learns kept names its dispel type (`0x4b3920`).
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
                row(589, |d| d.dispel = 1),
                row(980, |d| d.dispel = 2),
            ]
            .into(),
        );
        // The dispel list holds Magic (1), as a friendly Dispel Magic's learn left it.
        let book = crate::ui_action::PlayerActions {
            spells: [1243, 588, 19746, 20217].into(),
            dispels: vec![1],
            ..Default::default()
        };
        let mut pet = crate::ui_pet::PetBar::default();
        pet.spells.pet_guid = 0xF140_0000_0000_0077;
        pet.spells.spells = vec![benilla_protocol::messages::PetActionEntry::from(3110)];
        let raid = RaidFilter {
            book: Some(&book),
            pet: Some(&pet),
        };
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
        assert!(!debuff(980), "Curse, which the list lacks");
        let gone = RaidFilter {
            pet: Some(&crate::ui_pet::PetBar::default()),
            ..raid
        };
        assert!(!gone.keeps(Some(&catalog), 0, 3110), "no bar, no pet book");
        // The verdict rides on the list.
        let store = ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
            (47, 1243),
            (47 + 1, 1126),
            (95, 0x0000_0088),
        ]));
        let inputs: Vec<AuraInput> =
            other_unit_inputs(&store, Some(&catalog), raid, None).collect();
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
        assert_eq!(
            until_cancelled(Some(&ordinary), Some(&timed_row), true),
            Some(false)
        );
        assert_eq!(
            until_cancelled(Some(&ordinary), Some(&timed_row), false),
            Some(false)
        );
        assert_eq!(
            until_cancelled(Some(&ordinary), Some(&permanent_row), true),
            Some(true)
        );
        assert_eq!(
            until_cancelled(Some(&ordinary), Some(&scaling_row), true),
            Some(false),
            "a negative base with a positive per-level term is a SCALING duration, not permanence"
        );
        // No row: permanent (`0x4e4580`).
        assert_eq!(until_cancelled(Some(&ordinary), None, true), Some(true));

        // Clause 2: the area-aura override applies only to a non-cancelable aura (`0x4e4588`).
        assert_eq!(
            until_cancelled(Some(&area), Some(&timed_row), false),
            Some(true)
        );
        assert_eq!(
            until_cancelled(Some(&area), Some(&timed_row), true),
            Some(false),
            "a CANCELABLE aura skips the effect scan entirely"
        );

        // A catalog miss reads the expiry at the call.
        assert_eq!(until_cancelled(None, None, false), None);
    }
}
