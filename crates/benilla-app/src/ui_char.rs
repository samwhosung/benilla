//! The character window's half of the engine: the stat getters' reads of a unit's combat fields
//! ([`StatsObject`]), the active player's worn slots as the slot getters read them at the call
//! ([`worn_slot_view`], [`weapon_enchants`]), the inventory alerts' reads, the skill list
//! ([`SkillLive`]) and the skill tiers. [`unit_combat_stats`] is the half any unit shares: a
//! creature has no player fields, so those read absent and keep their defaults.

use bevy::prelude::*;

use benilla_protocol::messages::{ItemInfo, PLAYER_SKILL_SLOTS};
use benilla_protocol::ObjectFields;
use benilla_ui::script::{
    weapon_subclass_skill, CombatRead, InvSlotView, ItemRead, SkillEntry, UnitCombatStats,
    SKILL_DEFENSE, SKILL_UNARMED,
};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::{ClientCommand, NetCommands, ObjectStore};
use crate::query_cache::Ask;
use crate::ui_items::find_equip_slot;
use crate::ui_script::UiFeed;

mod skills_live;
pub(crate) use skills_live::SkillLive;

/// Equipment slots, 0-based (`EQUIPMENT_SLOT_*`): main hand, off hand, ranged.
const SLOT_MAIN_HAND: u8 = 15;
const SLOT_OFF_HAND: u8 = 16;
const SLOT_RANGED: u8 = 17;
/// Item class 2, weapon (vmangos `ItemPrototype.h:139`).
const ITEM_CLASS_WEAPON: u32 = 2;
/// Weapon subclass 19, wand (`ItemPrototype.h:211`), for `HasWandEquipped`.
const SUBCLASS_WAND: u32 = 19;

/// The character window's tables: the skill tiers.
pub(crate) struct UiCharPlugin;

impl Plugin for UiCharPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Startup,
            load_skill_tiers.after(benilla_assets::AssetSet::Open),
        )
        .add_systems(Update, feed_skill_tiers.in_set(UiFeed));
    }
}

/// `SkillTiers.dbc`, which `BuySkillTier` prices a step by.
#[derive(Resource)]
struct SkillTiersRes(benilla_formats::SkillTiers);

fn load_skill_tiers(mut commands: Commands, assets: Option<Res<benilla_assets::WorldAssets>>) {
    use benilla_assets::LockRecover;
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_skill_tiers(&mut chain)
    };
    match loaded {
        Ok(tiers) => commands.insert_resource(SkillTiersRes(tiers)),
        Err(e) => {
            warn!("ui_char: SkillTiers.dbc failed to load — BuySkillTier prices nothing: {e:#}")
        }
    }
}

/// `SkillTiers.dbc`, once per VM, which `BuySkillTier` prices a step by; the skill list reads the
/// slots through the call host ([`SkillLive`]).
fn feed_skill_tiers(
    mut hosted_vm: crate::script_calls::HostedVm,
    tiers: Option<Res<SkillTiersRes>>,
    mut tiers_pushed: Local<crate::ui_script::VmMemo<bool>>,
) {
    hosted_vm.run(|script| {
        let (Some(script), Some(tiers)) = (script, tiers.as_deref()) else {
            return;
        };
        let pushed = tiers_pushed.get(script);
        if !*pushed {
            script.set_skill_tiers(tiers.0.tiers());
            *pushed = true;
        }
    })
}

/// One skill slot as a Skills-tab row, or `None` where the reference's list build (`0x4d2cb0`)
/// skips it; the checks run in that build's order.
pub(crate) fn skills_row(
    slot: &benilla_protocol::messages::PlayerSkillSlot,
    catalog: &benilla_formats::SkillLineCatalog,
    race: u8,
    class: u8,
    level: u32,
) -> Option<SkillEntry> {
    if slot.skill_id == 0 {
        return None;
    }
    let id = u32::from(slot.skill_id);
    let line = catalog.line(id)?;
    let rc = catalog.race_class(id, race, class)?;
    // The category row must exist; no category filters, `Not Displayed` (12) included.
    let (category_name, category_order) = catalog.category(line.category_id)?;
    // Flag `0x2` hides `Dual Wield`, the racials, the mount lines and `GENERIC (DND)`.
    if rc.hidden() {
        return None;
    }
    // An untrained line shows only where the flags admit it at this level.
    if slot.value == 0 && !rc.displays_untrained(level) {
        return None;
    }
    Some(SkillEntry {
        skill_id: id,
        name: line.name.clone(),
        value: u32::from(slot.value),
        max: u32::from(slot.max),
        temp_bonus: i32::from(slot.temp_bonus),
        perm_bonus: i32::from(slot.perm_bonus),
        min_level: rc.min_level,
        cost_index: rc.cost_index,
        category_id: line.category_id,
        category_name: category_name.to_string(),
        category_order,
        description: line.description.clone(),
        // The unlearnable bit and a nonzero skill step (`0x4d3953`).
        abandonable: slot.step != 0 && rc.unlearnable(),
        mono: rc.mono(),
        step: u32::from(slot.step),
        flags: rc.flags,
        tier_id: rc.tier_id,
    })
}

/// The skills verbs' engine half, which the call host applies inside each call
/// ([`crate::script_calls::ScriptCallHost`]).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct SkillSteps<'w> {
    net: Res<'w, NetCommands>,
}

impl SkillSteps<'_> {
    /// `AbandonSkill`'s `CMSG_UNLEARN_SKILL` (`0x4d3b40`); nothing changes locally, as the
    /// server's `SetSkill(id, 0, 0)` (vmangos `SkillHandler.cpp:69`) returns as a skill-field
    /// update.
    pub(crate) fn apply(&mut self, call: benilla_ui::script::SkillCall) {
        let benilla_ui::script::SkillCall::Abandon(skill_id) = call;
        debug!("ui_char: abandon skill line {skill_id}");
        let _ = self.net.0.send(ClientCommand::UnlearnSkill { skill_id });
    }
}

/// The weapon-skill line for a worn item's record: vmangos's subclass table (`Item.cpp:700`) for
/// a weapon, Unarmed (162) for anything else, an empty slot, or a record in flight.
fn weapon_skill_id(record: Option<&ItemInfo>) -> u32 {
    record
        .filter(|t| t.class == ITEM_CLASS_WEAPON)
        .and_then(|t| weapon_subclass_skill(t.subclass))
        .unwrap_or(SKILL_UNARMED)
}

/// A skill's `(value + permanent bonus, temporary bonus)`, split as the reference's `0x5ea460`
/// splits it; value 0 skips the permanent add (`0x5ea4b4`). Signed: a malus can exceed the value.
fn skill_pair(store: &ObjectStore, skill_id: u32) -> (i32, i32) {
    for i in 0..PLAYER_SKILL_SLOTS {
        if let Some(s) = store.0.player_skill(i) {
            if u32::from(s.skill_id) == skill_id {
                let value = i32::from(s.value);
                let base = if value == 0 {
                    0
                } else {
                    value + i32::from(s.perm_bonus)
                };
                return (base, i32::from(s.temp_bonus));
            }
        }
    }
    (0, 0)
}

/// The active player's own half of its combat stats: its worn weapons, each slot's item resolved
/// by `entry` and its record only read, as the getter that asks reads it (`0x518f19`, `0x4c99c2`),
/// so the getters that read no weapon look none up.
pub(crate) struct Worn<'a> {
    /// The active player's store, whose seats name the items.
    store: &'a ObjectStore,
    /// An item's entry by guid, `None` for no object.
    entry: &'a dyn Fn(u64) -> Option<u32>,
    items: &'a Items,
}

impl<'a> Worn<'a> {
    /// The weapons `store`, the active player's, wears, each item looked up through `entry`.
    pub(crate) fn new(
        store: &'a ObjectStore,
        entry: &'a dyn Fn(u64) -> Option<u32>,
        items: &'a Items,
    ) -> Self {
        Worn {
            store,
            entry,
            items,
        }
    }

    /// The record of the item in 0-based `slot0`; `None` for an empty slot, an item not held or a
    /// record in flight.
    fn record(&self, slot0: u8) -> Option<&'a ItemInfo> {
        let guid = self.store.0.player_inv_slot(slot0)?;
        (self.entry)(guid).and_then(|e| self.items.template(e, 0, Ask::Read))
    }
}

/// A unit's combat stats as the stat getters read them ([`CombatRead`]): each field derived off
/// its descriptor as it is asked for, the active player's with its [`Worn`] weapons and its skill
/// and avoidance fields; any other unit's leave those at their defaults. A unit view's build takes
/// every field from here ([`unit_combat_stats`]), so the live read and the view are one
/// derivation.
pub(crate) struct StatsObject<'a> {
    store: &'a ObjectStore,
    own: Option<Worn<'a>>,
    /// [`Self::ranged_school`], once looked up: `UnitRangedDamage` reads three fields at it.
    school: std::cell::OnceCell<u8>,
}

impl<'a> StatsObject<'a> {
    /// Any unit's: its descriptor alone.
    pub(crate) fn unit(store: &'a ObjectStore) -> Self {
        StatsObject {
            store,
            own: None,
            school: Default::default(),
        }
    }

    /// The active player's, wearing `worn`.
    pub(crate) fn player(store: &'a ObjectStore, worn: Worn<'a>) -> Self {
        StatsObject {
            store,
            own: Some(worn),
            school: Default::default(),
        }
    }

    fn f(&self) -> &ObjectFields {
        &self.store.0
    }

    /// `UnitRangedDamage`'s school (`0x5189e1`-`0x518a45`): the ranged-slot item's record, read
    /// without asking, and its first damage line whose minimum is above 0, else school 0. The
    /// template keeps only lines whose maximum is above 0, which drops no well-formed such line.
    fn ranged_school(&self) -> u8 {
        *self.school.get_or_init(|| {
            self.own
                .as_ref()
                .and_then(|w| w.record(SLOT_RANGED))
                .and_then(|t| t.damages.iter().find(|d| d.min > 0.0))
                .map_or(0, |d| u8::try_from(d.school).unwrap_or(u8::MAX))
        })
    }

    /// The active player's avoidance chance `read` names; 0 for any other unit.
    fn own_chance(&self, read: impl FnOnce(&ObjectFields) -> Option<f32>) -> f32 {
        self.own
            .as_ref()
            .and_then(|_| read(self.f()))
            .unwrap_or(0.0)
    }

    /// The active player's pair for `skill_id`; `(0, 0)` for any other unit.
    fn own_skill(&self, skill_id: impl FnOnce(&Worn<'_>) -> Option<u32>) -> (i32, i32) {
        self.own
            .as_ref()
            .and_then(skill_id)
            .map_or((0, 0), |id| skill_pair(self.store, id))
    }
}

impl CombatRead for StatsObject<'_> {
    fn stats(&self, i: usize) -> i32 {
        self.f().unit_stat(i as u8).unwrap_or(0) as i32
    }
    // The buff-split arrays are integers on the wire, not floats.
    fn stat_pos(&self, i: usize) -> i32 {
        self.f().player_posstat(i as u8).unwrap_or(0)
    }
    fn stat_neg(&self, i: usize) -> i32 {
        self.f().player_negstat(i as u8).unwrap_or(0)
    }
    fn resistances(&self, i: usize) -> i32 {
        self.f().unit_resistance(i as u8).unwrap_or(0)
    }
    fn resistance_pos(&self, i: usize) -> i32 {
        self.f().player_resistance_buff_pos(i as u8).unwrap_or(0)
    }
    fn resistance_neg(&self, i: usize) -> i32 {
        self.f().player_resistance_buff_neg(i as u8).unwrap_or(0)
    }
    fn min_damage(&self) -> f32 {
        self.f().unit_min_damage().unwrap_or(0.0)
    }
    fn max_damage(&self) -> f32 {
        self.f().unit_max_damage().unwrap_or(0.0)
    }
    fn min_offhand_damage(&self) -> f32 {
        self.f().unit_min_offhand_damage().unwrap_or(0.0)
    }
    fn max_offhand_damage(&self) -> f32 {
        self.f().unit_max_offhand_damage().unwrap_or(0.0)
    }
    fn physical_bonus_pos(&self) -> i32 {
        self.f().player_mod_damage_done_pos(0).unwrap_or(0)
    }
    fn physical_bonus_neg(&self) -> i32 {
        self.f().player_mod_damage_done_neg(0).unwrap_or(0)
    }
    fn damage_percent(&self) -> f32 {
        self.f().player_mod_damage_done_pct(0).unwrap_or(1.0)
    }
    // The three at the ranged school (`0x518a77`, `0x518ac0`, `0x518b0e`).
    fn ranged_bonus_pos(&self) -> i32 {
        self.f()
            .player_mod_damage_done_pos(self.ranged_school())
            .unwrap_or(0)
    }
    fn ranged_bonus_neg(&self) -> i32 {
        self.f()
            .player_mod_damage_done_neg(self.ranged_school())
            .unwrap_or(0)
    }
    fn ranged_percent(&self) -> f32 {
        self.f()
            .player_mod_damage_done_pct(self.ranged_school())
            .unwrap_or(1.0)
    }
    // As the fields stand, 0 when unsent (`0x518ea0`).
    fn main_attack_time_ms(&self) -> u32 {
        self.f().unit_base_attack_time(0).unwrap_or(0)
    }
    fn offhand_attack_time_ms(&self) -> u32 {
        self.f().unit_base_attack_time(1).unwrap_or(0)
    }
    // The off hand counts only as a weapon (a shield does not swing); false until the record
    // lands.
    fn has_offhand(&self) -> bool {
        self.own
            .as_ref()
            .and_then(|w| w.record(SLOT_OFF_HAND))
            .is_some_and(|t| t.class == ITEM_CLASS_WEAPON)
    }
    fn attack_power(&self) -> i32 {
        self.f().unit_attack_power().unwrap_or(0)
    }
    fn attack_power_pos(&self) -> i32 {
        i32::from(self.f().unit_attack_power_mods().0)
    }
    fn attack_power_neg(&self) -> i32 {
        i32::from(self.f().unit_attack_power_mods().1)
    }
    fn attack_power_multiplier(&self) -> f32 {
        self.f().unit_attack_power_multiplier().unwrap_or(0.0)
    }
    fn ranged_attack_power(&self) -> i32 {
        self.f().unit_ranged_attack_power().unwrap_or(0)
    }
    fn ranged_attack_power_pos(&self) -> i32 {
        i32::from(self.f().unit_ranged_attack_power_mods().0)
    }
    fn ranged_attack_power_neg(&self) -> i32 {
        i32::from(self.f().unit_ranged_attack_power_mods().1)
    }
    fn ranged_attack_power_multiplier(&self) -> f32 {
        self.f()
            .unit_ranged_attack_power_multiplier()
            .unwrap_or(0.0)
    }
    fn ranged_attack_time_ms(&self) -> u32 {
        self.f().unit_ranged_attack_time().unwrap_or(0)
    }
    fn ranged_min_damage(&self) -> f32 {
        self.f().unit_min_ranged_damage().unwrap_or(0.0)
    }
    fn ranged_max_damage(&self) -> f32 {
        self.f().unit_max_ranged_damage().unwrap_or(0.0)
    }
    fn main_weapon_skill(&self) -> (i32, i32) {
        self.own_skill(|w| Some(weapon_skill_id(w.record(SLOT_MAIN_HAND))))
    }
    // `UnitAttackBothHands`'s second pair; a shield or an empty off hand reads Unarmed.
    fn offhand_weapon_skill(&self) -> (i32, i32) {
        self.own_skill(|w| Some(weapon_skill_id(w.record(SLOT_OFF_HAND))))
    }
    fn ranged_weapon_skill(&self) -> (i32, i32) {
        self.own_skill(|w| {
            w.record(SLOT_RANGED)
                .filter(|t| t.class == ITEM_CLASS_WEAPON)
                .and_then(|t| weapon_subclass_skill(t.subclass))
        })
    }
    // `UnitDefense`'s pair. The 1.12 player sheet has no Defense row: its `SKILL_LINES_CHANGED`
    // arm (`PaperDollFrame.lua:70-72`) redraws only the Attack row.
    fn defense_skill(&self) -> (i32, i32) {
        self.own_skill(|_| Some(SKILL_DEFENSE))
    }
    fn has_wand(&self) -> bool {
        self.own
            .as_ref()
            .and_then(|w| w.record(SLOT_RANGED))
            .is_some_and(|t| t.class == ITEM_CLASS_WEAPON && t.subclass == SUBCLASS_WAND)
    }
    // `GetDodgeChance`/`GetParryChance`/`GetBlockChance`: player fields, already percentages.
    fn dodge_percent(&self) -> f32 {
        self.own_chance(ObjectFields::player_dodge_percentage)
    }
    fn parry_percent(&self) -> f32 {
        self.own_chance(ObjectFields::player_parry_percentage)
    }
    fn block_percent(&self) -> f32 {
        self.own_chance(ObjectFields::player_block_percentage)
    }
}

/// Any unit's combat stats off its descriptor alone ([`StatsObject::unit`]).
pub(crate) fn unit_combat_stats(store: &ObjectStore) -> UnitCombatStats {
    UnitCombatStats::read(&StatsObject::unit(store))
}

/// The active player's stats as one value: [`unit_combat_stats`] plus the equipment flags and the
/// skill pairs ([`StatsObject::player`]), for a test of their derivation.
#[cfg(test)]
fn combat_stats(
    store: &ObjectStore,
    objects: &crate::net::Objects,
    items: &Items,
) -> UnitCombatStats {
    let entry = |guid| objects.object(guid)?.object_entry();
    UnitCombatStats::read(&StatsObject::player(store, Worn::new(store, &entry, items)))
}

/// One slot's view from the item object `guid` and its countdowns, off `held`, and its record,
/// read without asking (no icon while in flight).
#[allow(clippy::too_many_arguments)] // the slot's inputs, each a store it reads
fn slot_view(
    held: &dyn crate::ui_items::ItemObjects,
    items: &Items,
    icons: Option<&ItemDisplays>,
    rolls: crate::items::RollCatalogs,
    names: &crate::names::NameCache,
    // `ItemSubClass.dbc`, for the count gate below; absent, every bag counts 0.
    sub_classes: Option<&crate::ui_items::ItemSubClasses>,
    // The player's `ChrClasses.dbc` relic flag, which decides `find_equip_slot`'s INVSLOT 17.
    has_relic_slot: bool,
    guid: u64,
) -> Option<InvSlotView> {
    let countdowns = held.countdowns(guid);
    let enchant_ms: [Option<u64>; crate::items::ENCHANT_SLOTS] =
        std::array::from_fn(|s| countdowns.and_then(|c| c.enchant_remaining_display_ms(s as u32)));
    let duration_ms = countdowns.and_then(|c| c.lifetime_remaining_display_ms());
    let obj = held.object(guid)?;
    let entry = obj.object_entry()?;
    // What the getters read of it as they run, read here the same way.
    let live = crate::ui_items::live::LiveItem::object(obj, guid, items, icons, rolls);
    let count = live.stack_count().unwrap_or(1).max(1);
    // `OBJECT_FIELD_TYPE & TYPEMASK_CONTAINER`, read off the object as `GetInventoryItemCount`'s
    // first fork reads it (`0x4c87a6`).
    let is_container = obj.is_container();
    let content_guids: Vec<u64> = {
        let slots = obj.container_num_slots().unwrap_or(0).min(36) as u8;
        (0..slots).filter_map(|s| obj.container_slot(s)).collect()
    };
    let durability = live.durability();
    // `ITEM_FIELD_FLAGS`, for its wrapped (0x08) and force-red (0x10) bits.
    let flags = obj.item_flags().unwrap_or(0);
    // Soulbound or carrying a binding enchant (`0x5da2c0`), read off the raw descriptor, never off
    // the enchant lines below.
    let already_bound = crate::items::already_bound(
        obj,
        crate::items::record_flags(items, entry, guid),
        rolls.enchants,
    );
    // Only read: the tooltip build asks the creator in its call
    // ([`benilla_ui::script::RecordAsk::ItemBuildName`]).
    let creator = obj
        .item_creator()
        .filter(|&g| g != 0)
        .and_then(|g| names.resolve_player(g, Ask::Read).map(str::to_string));
    let gift_creator = crate::items::gift_creator_name(obj, names);
    let (creator_guid, gift_creator_guid) = crate::items::creator_guids(obj);
    // All seven enchant slots: our own gear streams as item objects, where an inspected player's
    // descriptor carries only two.
    let enchants = crate::items::enchant_lines(
        (0..7).map(|s| {
            (
                s,
                obj.item_enchant(s).unwrap_or(0),
                obj.item_enchant_charges(s),
                enchant_ms[usize::from(s)],
            )
        }),
        rolls.enchants,
    );
    // Only read, as the inventory getters read it (`0x4c8a0a`, `0x4c8d34`): the item's create asked.
    let t = items.template(entry, guid, Ask::Read);
    let (equip_slots, kind, bar_placeable, consumable) = match t {
        Some(t) => (
            find_equip_slot(t.inventory_type, has_relic_slot),
            (t.class, t.subclass),
            t.placeable_on_action_bar(),
            t.is_consumable(),
        ),
        None => (Vec::new(), (0, 0), false, false),
    };
    // A container counts its contents' stacks when its subclass's `DisplayFlags` has 0x4, else 0
    // (`GetInventoryItemCount` `0x4c8680`: `0x4c881a`, summed by `0x622130`); the bit is on Soul
    // Bag (1/1) and the Quiver class (11/0..3) only, so a plain bag shows no count.
    let counts_contents = sub_classes.is_some_and(|c| c.0.display_flags(kind.0, kind.1) & 0x4 != 0);
    let contents_count = is_container.then(|| {
        if counts_contents {
            content_guids
                .iter()
                .filter_map(|&g| held.object(g))
                .map(|o| o.item_stack_count().unwrap_or(1).max(1))
                .sum()
        } else {
            0
        }
    });
    Some(InvSlotView {
        roll: Default::default(),
        inventory_type: live.cached().then(|| live.inventory_type()),
        bonding: t.map_or(0, |t| t.bonding),
        // A container's filled seats, which `ClickTradeButton` counts (`0x4bff50`).
        occupied_seats: is_container
            .then(|| content_guids.iter().filter(|&&g| g != 0).count() as u32),
        use_spell: t.and_then(|t| t.use_spell).map_or(0, |u| u.spell_id),
        spell_0: t.is_some_and(benilla_protocol::ItemInfo::has_spell_0),
        spell_charges: std::array::from_fn(|i| obj.item_spell_charges(i as u8).unwrap_or(0)),
        use_fork: t.map_or(Default::default(), |t| {
            crate::ui_items::dispatch_fork(t, flags, obj.item_text_id().unwrap_or(0))
        }),
        item_id: entry,
        icon: live.texture(),
        count,
        contents_count,
        quality: live.quality().map_or(0, |q| q as i32),
        // The item object's name (`0x5d8bc0`): the roll's suffix, unless it is wrapped.
        name: live.name(),
        // `GetInventoryItemLink` links the player's own item object (`0x4c8ce2` → `0x52ae00`),
        // its record cached or not.
        uncached_link: live.uncached_link(),
        link: live.link(),
        durability,
        flags,
        already_bound,
        guid,
        equip_slots,
        bar_placeable,
        consumable,
        creator,
        gift_creator,
        creator_guid,
        gift_creator_guid,
        enchants,
        duration_ms,
        field_duration: obj.item_duration().unwrap_or(0),
    })
}

/// Carried ammo matching `ammo_id`, in the backpack and the four equipped bags; bank and buyback
/// never count.
fn ammo_count(me: &ObjectFields, held: &dyn crate::ui_items::ItemObjects, ammo_id: u32) -> u32 {
    let mut n = 0;
    let mut add = |guid: Option<u64>| {
        if let Some(o) = guid.and_then(|g| held.object(g)) {
            if o.object_entry() == Some(ammo_id) {
                n += o.item_stack_count().unwrap_or(1);
            }
        }
    };
    for i in 0..16u8 {
        add(me.player_pack_slot(i));
    }
    for b in 19..23u8 {
        if let Some(bag) = me.player_inv_slot(b).and_then(|g| held.object(g)) {
            let slots = bag.container_num_slots().unwrap_or(0).min(36) as u8;
            for s in 0..slots {
                if let Some(o) = bag.container_slot(s).and_then(|g| held.object(g)) {
                    if o.object_entry() == Some(ammo_id) {
                        n += o.item_stack_count().unwrap_or(1);
                    }
                }
            }
        }
    }
    n
}

/// The 0-based equipment slots of `0x4c7ee0`'s 11 worn regions, in `0x806eb8`'s order: head,
/// shoulders, chest, waist, legs, feet, wrists, hands, main hand, off hand and ranged.
const ALERT_REGIONS: [u8; 11] = [0, 2, 4, 5, 6, 7, 8, 9, 15, 16, 17];

/// What the alert recompute `0x4c7ee0` reads off the active player `me`, each item found through
/// `items`: the item in each worn region's slot (`0x6228a0`), its record's flags as cached
/// (`records`, 0 uncached), and its temporary enchantment's milliseconds left (`left`, by guid);
/// and with `PLAYER_AMMO_ID` set (`[+0xe68]+0x102c`, `0x4c7f53`), the carried count of that
/// ammunition (`0x622130(id, 0)`, `0x4c7f73`).
pub(crate) fn alert_inputs(
    me: &ObjectFields,
    items: &(impl crate::ui_items::ItemObjects + ?Sized),
    records: &Items,
    left: impl Fn(u64) -> u32,
) -> benilla_ui::script::AlertInputs {
    let worn = ALERT_REGIONS.map(|slot| {
        let guid = me.player_inv_slot(slot).filter(|&g| g != 0)?;
        let item = items.object(guid).filter(|f| f.is_item())?;
        let record_flags = item
            .object_entry()
            .and_then(|entry| records.template(entry, 0, Ask::Read))
            .map_or(0, |t| t.flags);
        Some(benilla_ui::script::AlertItem {
            flags: item.item_flags().unwrap_or(0),
            durability: item.item_durability().unwrap_or(0),
            max_durability: item.item_max_durability().unwrap_or(0),
            record_flags,
            temp_enchant: item.item_enchant(TEMP_ENCHANTMENT_SLOT).unwrap_or(0) as u32,
            temp_enchant_left_ms: left(guid),
        })
    });
    let ammo = me.player_ammo_id().filter(|&id| id != 0).map(|id| {
        crate::ui_items::count_of(me, items, id, crate::ui_items::InventoryScope::DEFAULT)
    });
    benilla_ui::script::AlertInputs { worn, ammo }
}

/// The active player `me`'s item at live-API id `slot` as the slot getters read it at the call
/// ([`benilla_ui::script::WornSlotQuery`]): the reference's look it up in the player's item list
/// as they run (`GetItemInSlot` `0x6228a0`; `GetInventoryItemLink` at `0x4c8cd3`). 0 is the ammo,
/// off `PLAYER_AMMO_ID` and its record; 1..=19 the equipment and 20..=23 the equipped bags
/// themselves, inventory slot `slot - 1`; 64..=69 the bank bags, which the stock bank reads through
/// `BankButtonIDToInvSlotID(i, 1)` (`BankFrame.lua:28-35, 199-204`). `None` for an empty slot, an
/// object `held` lacks, or any other id. Each object and its countdowns are read off `held`, as of
/// the getter's place.
#[allow(clippy::too_many_arguments)] // the slot's inputs, each a store it reads
pub(crate) fn worn_slot_view(
    me: &ObjectFields,
    held: &dyn crate::ui_items::ItemObjects,
    items: &Items,
    icons: Option<&ItemDisplays>,
    rolls: crate::items::RollCatalogs,
    names: &crate::names::NameCache,
    sub_classes: Option<&crate::ui_items::ItemSubClasses>,
    classes: Option<&benilla_formats::ChrClasses>,
    slot: usize,
) -> Option<InvSlotView> {
    let view = |guid: u64, has_relic_slot: bool| {
        slot_view(
            held,
            items,
            icons,
            rolls,
            names,
            sub_classes,
            has_relic_slot,
            guid,
        )
    };
    match slot {
        0 => ammo_view(me, held, items, icons, rolls),
        1..=23 => {
            let guid = me.player_inv_slot(slot as u8 - 1).filter(|&g| g != 0)?;
            let has_relic_slot = me
                .unit_class()
                .is_some_and(|c| classes.is_some_and(|t| t.has_relic_slot(u32::from(c))));
            view(guid, has_relic_slot)
        }
        // A bank bag slot holds a bag; the relic flag decides INVSLOT 17 alone.
        64..=69 => view(
            me.player_bank_bag_slot((slot - 64) as u8)
                .filter(|&g| g != 0)?,
            false,
        ),
        _ => None,
    }
}

/// Each of `me`'s slots 0..=23 as [`worn_slot_view`] would find it, by its object guid and entry
/// off `held` and with no view built ([`benilla_ui::script::WornKeysQuery`]); the ammo slot is
/// `PLAYER_AMMO_ID` with guid 0.
pub(crate) fn worn_keys(
    me: &ObjectFields,
    held: &dyn crate::ui_items::ItemObjects,
) -> benilla_ui::script::WornKeys {
    std::array::from_fn(|slot| match slot {
        0 => me.player_ammo_id().filter(|&id| id != 0).map(|id| (0, id)),
        _ => {
            let guid = me.player_inv_slot(slot as u8 - 1).filter(|&g| g != 0)?;
            Some((guid, held.object(guid)?.object_entry()?))
        }
    })
}

/// The ammo slot's view: `PLAYER_AMMO_ID`'s record, read without asking, and the carried count;
/// `None` with no ammo set. What asks the record are the texture and quality getters' lookups
/// with the callback `0x4c84f0` (`0x4c83ce`, `0x4c8a0a`).
fn ammo_view(
    me: &ObjectFields,
    held: &dyn crate::ui_items::ItemObjects,
    items: &Items,
    icons: Option<&ItemDisplays>,
    rolls: crate::items::RollCatalogs,
) -> Option<InvSlotView> {
    let ammo_id = me.player_ammo_id().filter(|&id| id != 0)?;
    let live =
        crate::ui_items::live::LiveItem::record(ammo_id, Default::default(), items, icons, rolls);
    Some(InvSlotView {
        uncached_link: None,
        inventory_type: live.cached().then(|| live.inventory_type()),
        durability: None,
        flags: 0,
        // Ammo has no item object, only its template, so there is no bind to read.
        already_bound: false,
        bonding: 0,
        occupied_seats: None,
        use_spell: 0,
        use_fork: Default::default(),
        item_id: ammo_id,
        icon: live.texture(),
        count: ammo_count(me, held, ammo_id),
        // `GetInventoryItemCount`'s `PLAYER_AMMO_ID` leg, before the container fork.
        contents_count: None,
        quality: live.quality().map_or(0, |q| q as i32),
        name: live.name(),
        link: live.link(),
        roll: Default::default(),
        // The ammo leg names the ammo by entry, never by an item object (`0x4c7509`): no guid
        // to lock, and no equip slots, as `CursorCanGoInSlot(0)` answers held ammo alone.
        guid: 0,
        equip_slots: Vec::new(),
        bar_placeable: false,
        consumable: false,
        creator: None,
        gift_creator: None,
        creator_guid: 0,
        gift_creator_guid: 0,
        // No ammo instance streams, only its template id: no enchants, no countdown.
        enchants: Vec::new(),
        duration_ms: None,
        field_duration: 0,
        spell_0: false,
        spell_charges: [0; 5],
    })
}

/// The ammo slot getters' lookup with the callback `0x4c84f0` (`0x4c83bc`, `0x4c89cf`), inside
/// their calls ([`benilla_ui::script::RecordAsk::AmmoItem`]): a miss asks, a refused entry again,
/// and waits for the answer, whose ready record signals `UNIT_INVENTORY_CHANGED` (`0x4c8509`) at
/// its packet. A hit registers no callback: the call reads the record itself.
pub(crate) fn ask_ammo_record(items: &Items, commands: &NetCommands, entry: u32, owner: u64) {
    if items.template(entry, owner, Ask::Event(commands)).is_none() {
        items.wait_for_ammo_record(entry);
    }
}

/// The 0-based weapon slots `GetWeaponEnchantInfo` (`0x4c9790`) reads (`0x4c97c3`, `0x4c988b`);
/// the frames' live ids are one higher.
const EQUIPMENT_SLOT_MAINHAND: u8 = 15;
const EQUIPMENT_SLOT_OFFHAND: u8 = 16;

/// The temporary enchantment slot (0 permanent, 2..6 random properties), as the reference's
/// remaining-time read passes it (`0x4c9828`).
const TEMP_ENCHANTMENT_SLOT: u8 = 1;

/// `GetWeaponEnchantInfo`'s two weapons as the call reads them
/// ([`benilla_ui::script::WeaponEnchantQuery`]): the active player `me`'s main and off hands in
/// its item list (`0x6228a0` at `0x4c97ca`), each object and its deadline off `held`, the
/// remaining time recomputed per call (`0x5d9d00`).
pub(crate) fn weapon_enchants(
    me: &ObjectFields,
    held: &dyn crate::ui_items::ItemObjects,
) -> [Option<benilla_ui::script::WeaponEnchant>; 2] {
    [EQUIPMENT_SLOT_MAINHAND, EQUIPMENT_SLOT_OFFHAND].map(|slot0| weapon_enchant(me, held, slot0))
}

/// One weapon's temporary enchantment, read off the raw descriptor rather than
/// [`InvSlotView::enchants`], which drops unnamed ids and the totem imbues; the reference's only
/// gate is a nonzero id (`[descriptor+0x4c]`).
fn weapon_enchant(
    me: &ObjectFields,
    held: &dyn crate::ui_items::ItemObjects,
    slot0: u8,
) -> Option<benilla_ui::script::WeaponEnchant> {
    let guid = me.player_inv_slot(slot0)?;
    // The deadline read, not the tooltip's: an elapsed timer answers 0, not "no timer".
    let remaining_ms = held
        .countdowns(guid)
        .and_then(|c| c.enchant_deadline_ms(u32::from(TEMP_ENCHANTMENT_SLOT)));
    let obj = held.object(guid)?;
    obj.item_enchant(TEMP_ENCHANTMENT_SLOT)?;
    Some(benilla_ui::script::WeaponEnchant {
        remaining_ms,
        charges: obj.item_enchant_charges(TEMP_ENCHANTMENT_SLOT),
    })
}

/// The doll's copies a getter with no host reads, seated off the active player as it stands: its
/// combat stats, its worn slots and its weapons' enchantments, for a test whose reads run with no
/// host lent. In game the getters read them at the call ([`benilla_ui::script::WornSlotQuery`]).
#[cfg(test)]
pub(crate) fn seat_doll_once(world: &mut World) {
    use bevy::ecs::system::RunSystemOnce;
    world
        .run_system_once(seat_doll)
        .expect("the doll's seat runs as a one-shot system");
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)] // the doll's inputs, each a store it reads
fn seat_doll(
    mut vm: NonSendMut<benilla_ui::script::UiScript>,
    objects: crate::net::Objects,
    me: Query<&ObjectStore, With<crate::net::SelfPlayer>>,
    items: Res<Items>,
    icons: Option<Res<ItemDisplays>>,
    rolls: crate::items::Rolls,
    names: Res<crate::names::NameCache>,
    sub_classes: Option<Res<crate::ui_items::ItemSubClasses>>,
    classes: Option<Res<crate::chr_classes::ChrClassTable>>,
) {
    let Ok(store) = me.single() else {
        return;
    };
    vm.set_player_combat_stats(Some(combat_stats(store, &objects, &items)));
    let view = |slot: usize| {
        worn_slot_view(
            &store.0,
            &objects,
            &items,
            icons.as_deref(),
            rolls.get(),
            &names,
            sub_classes.as_deref(),
            classes.as_deref().map(|t| &t.0),
            slot,
        )
    };
    vm.set_inventory_slots(std::array::from_fn(view));
    vm.set_bank_bag_slots(std::array::from_fn(|i| view(64 + i)));
    let [main_hand, off_hand] = weapon_enchants(&store.0, &objects);
    vm.set_weapon_enchants(main_hand, off_hand);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::SelfPlayer;
    use benilla_ui::script::UiScript;

    /// A worn bag's view says how many of its seats are filled, which `ClickTradeButton` reads
    /// to refuse it (`0x4bff50`, then `0x4bffae` for an empty one in a bag slot).
    #[test]
    fn a_worn_bag_view_counts_its_filled_seats() {
        use benilla_protocol::field::{FIELD_CONTAINER_SLOT_1, FIELD_OBJECT_ENTRY};
        let mut objs = crate::ui_items::TestObjects::new();
        // `OBJECT_FIELD_TYPE` container, `CONTAINER_FIELD_NUM_SLOTS` 4, seat 2 filled.
        objs.spawn_container(
            0xB1,
            ObjectFields::from_pairs(&[
                (2, 0x7),
                (FIELD_OBJECT_ENTRY, 4496),
                (48, 4),
                (FIELD_CONTAINER_SLOT_1 + 4, 0xC1),
            ]),
        );
        objs.spawn_container(
            0xB2,
            ObjectFields::from_pairs(&[(2, 0x7), (FIELD_OBJECT_ENTRY, 4496), (48, 4)]),
        );
        objs.spawn(
            0xB3,
            ObjectFields::from_pairs(&[(2, 0x3), (FIELD_OBJECT_ENTRY, 2589)]),
        );
        let items = Items::default();
        let names = crate::names::NameCache::default();
        let objects = objs.get();
        let seats = |guid: u64| {
            slot_view(
                &objects,
                &items,
                None,
                crate::items::RollCatalogs::NONE,
                &names,
                None,
                false,
                guid,
            )
            .expect("a view")
            .occupied_seats
        };
        assert_eq!(seats(0xB1), Some(1));
        assert_eq!(seats(0xB2), Some(0));
        assert_eq!(seats(0xB3), None);
    }
    use benilla_protocol::messages::ObjectFields;

    /// The reference's split (`0x5ea460`); a value-0 line skips the permanent add (`0x5ea4b4`).
    #[test]
    fn a_skill_pair_folds_the_permanent_bonus_into_the_base() {
        /// `PLAYER_SKILL_INFO_1_1`: id|step, value|max, temp|perm per slot.
        const F_SKILL: u16 = 718;
        let pair = |lo: u16, hi: u16| u32::from(lo) | (u32::from(hi) << 16);
        // Slot 0: Defense (95) at 300, +10 temporary, +5 permanent.
        // Slot 1: Swords (43) at 0, with a +7 permanent that must not surface.
        let store = ObjectStore(ObjectFields::from_pairs(&[
            (F_SKILL, pair(95, 0)),
            (F_SKILL + 1, pair(300, 300)),
            (F_SKILL + 2, pair(10, 5)),
            (F_SKILL + 3, pair(43, 0)),
            (F_SKILL + 4, pair(0, 300)),
            (F_SKILL + 5, pair(0, 7)),
        ]));
        assert_eq!(
            skill_pair(&store, 95),
            (305, 10),
            "the permanent bonus belongs to the base and the temporary one is the modifier"
        );
        assert_eq!(
            skill_pair(&store, 43),
            (0, 0),
            "a line at value 0 skips the perm add — a bare talent bonus is not a skill"
        );
        assert_eq!(
            skill_pair(&store, 162),
            (0, 0),
            "a line the player does not have at all"
        );
        // A malus deeper than the skill is representable, which is why the pair is signed.
        let cursed = ObjectStore(ObjectFields::from_pairs(&[
            (F_SKILL, pair(95, 0)),
            (F_SKILL + 1, pair(5, 300)),
            (F_SKILL + 2, pair(0, u16::MAX - 9)), // perm = -10
        ]));
        assert_eq!(skill_pair(&cursed, 95), (-5, 0));
    }

    /// `PLAYER_FIELD_BANK_BAG_SLOT_1`, bank bag slot 0's guid pair.
    const F_BANK_BAG_1: u16 = 612;
    /// `OBJECT_FIELD_ENTRY` on the item object.
    const F_OBJECT_ENTRY: u16 = 3;
    /// Any container entry ("Traveler's Backpack"); the view only needs it to resolve.
    const BAG_ENTRY: u32 = 4500;
    const BAG: u64 = 0x4000_0000_0000_0abc;

    /// Re-stream a live item object's fields as a values update does.
    fn restream(app: &mut App, guid: u64, fields: ObjectFields) {
        let entity = app
            .world()
            .resource::<crate::net::GuidIndex>()
            .0
            .get(&guid)
            .copied()
            .expect("the item was streamed");
        *app.world_mut().get_mut::<ObjectStore>(entity).unwrap() = ObjectStore(fields);
    }

    /// A world with `bag`, or nothing, in the first bank bag slot, and a VM recording events.
    fn world_with_bank_bag(bag: Option<u64>) -> App {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<Items>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<crate::names::NameCache>()
            .insert_resource(NetCommands(tx));
        let fields = match bag {
            Some(g) => ObjectFields::from_pairs(&[
                (F_BANK_BAG_1, g as u32),
                (F_BANK_BAG_1 + 1, (g >> 32) as u32),
            ]),
            None => ObjectFields::from_pairs(&[(F_BANK_BAG_1, 0), (F_BANK_BAG_1 + 1, 0)]),
        };
        app.world_mut().spawn((SelfPlayer, ObjectStore(fields)));
        if let Some(g) = bag {
            // The bag is an item entity; its template lives on `Items`.
            crate::items::test_spawn_item(
                app.world_mut(),
                g,
                ObjectFields::from_pairs(&[(F_OBJECT_ENTRY, BAG_ENTRY)]),
                true,
            );
            app.world_mut().resource_mut::<Items>().insert_template(
                BAG_ENTRY,
                Some(crate::items::test_template("Traveler's Backpack")),
            );
        }
        let script = UiScript::new().unwrap();
        script
            .run(
                r#"
                SEEN = {}
                local f = CreateFrame("Frame")
                f:RegisterEvent("PLAYERBANKSLOTS_CHANGED")
                f:RegisterEvent("UNIT_INVENTORY_CHANGED")
                f:SetScript("OnEvent", function()
                    table.insert(SEEN, event .. " " .. tostring(arg1))
                end)
            "#,
            )
            .unwrap();
        app.insert_non_send_resource(script);
        app
    }

    /// `chunk` with the player's items lent as the getters read them at the call.
    fn eval<T: benilla_ui::mlua::FromLuaMulti>(app: &mut App, chunk: &str) -> T {
        crate::ui_items::eval_with_bags(app.world_mut(), chunk)
    }

    /// Rewrite the active player's descriptor wholesale: `ObjectFields` has no setter.
    fn set_self(app: &mut App, pairs: &[(u16, u32)]) {
        let mut q = app
            .world_mut()
            .query_filtered::<&mut ObjectStore, With<SelfPlayer>>();
        let mut store = q.single_mut(app.world_mut()).unwrap();
        *store = ObjectStore(ObjectFields::from_pairs(pairs));
    }

    /// The doll reads its item at the call (`GetInventoryItemBroken` → `0x6228a0`): a repair shows
    /// in the next call, with no push between.
    #[test]
    fn a_repair_reads_at_the_next_call() {
        use benilla_protocol::field::FIELD_PLAYER_INV_SLOT_HEAD;
        const HELM: u64 = 0x4000_0000_0000_0b01;
        const HELM_ID: u32 = 7366;
        /// `ITEM_FIELD_DURABILITY` and `_MAXDURABILITY`.
        const DURABILITY: u16 = 46;
        let mut app = world_with_bank_bag(None);
        set_self(
            &mut app,
            &[
                (FIELD_PLAYER_INV_SLOT_HEAD, HELM as u32),
                (FIELD_PLAYER_INV_SLOT_HEAD + 1, (HELM >> 32) as u32),
            ],
        );
        crate::items::test_spawn_item(
            app.world_mut(),
            HELM,
            ObjectFields::from_pairs(&[(F_OBJECT_ENTRY, HELM_ID), (DURABILITY, 0), (47, 40)]),
            false,
        );
        app.world_mut()
            .resource_mut::<Items>()
            .insert_template(HELM_ID, Some(crate::items::test_template("Helm")));
        let broken = r#"return GetInventoryItemBroken("player", 1) ~= nil"#;
        assert!(eval::<bool>(&mut app, broken));
        restream(
            &mut app,
            HELM,
            ObjectFields::from_pairs(&[(F_OBJECT_ENTRY, HELM_ID), (DURABILITY, 40), (47, 40)]),
        );
        assert!(!eval::<bool>(&mut app, broken), "the doll reads the repair");
    }

    /// The player's own equipped link comes from the item object through `0x52ae00`
    /// (`GetInventoryItemLink` at `0x4c8ce2`): the permanent enchant (`+0x40`), the random
    /// property id (`+0x98`) and the property seed (`+0x94`), in `0x52adb0`'s order.
    #[test]
    fn an_equipped_link_carries_the_enchant_the_roll_and_the_seed() {
        use benilla_protocol::field::FIELD_PLAYER_INV_SLOT_HEAD;
        const HELM: u64 = 0x4000_0000_0000_0b02;
        const HELM_ID: u32 = 7366;
        let mut app = world_with_bank_bag(None);
        set_self(
            &mut app,
            &[
                (FIELD_PLAYER_INV_SLOT_HEAD, HELM as u32),
                (FIELD_PLAYER_INV_SLOT_HEAD + 1, (HELM >> 32) as u32),
            ],
        );
        crate::items::test_spawn_item(
            app.world_mut(),
            HELM,
            ObjectFields::from_pairs(&[(F_OBJECT_ENTRY, HELM_ID), (22, 1900), (43, 7), (44, 862)]),
            false,
        );
        app.world_mut()
            .resource_mut::<Items>()
            .insert_template(HELM_ID, Some(crate::items::test_template("Helm")));
        let link: String = eval(&mut app, r#"return GetInventoryItemLink("player", 1)"#);
        assert_eq!(link, "|cffffffff|Hitem:7366:1900:862:7|h[Helm]|h|r");
    }

    /// An own item whose record the cache lacks still links: `0x52ae00` answers null only for a
    /// null object (`0x52ae24`), takes quality 1 without a record (`0x52aec5`), and the name
    /// builder leaves the name empty (`0x5d8b25`, the miss at `0x5d8b40`).
    #[test]
    fn an_equipped_item_with_no_record_links_white_and_nameless() {
        use benilla_protocol::field::FIELD_PLAYER_INV_SLOT_HEAD;
        const HELM: u64 = 0x4000_0000_0000_0b02;
        const HELM_ID: u32 = 7366;
        let mut app = world_with_bank_bag(None);
        set_self(
            &mut app,
            &[
                (FIELD_PLAYER_INV_SLOT_HEAD, HELM as u32),
                (FIELD_PLAYER_INV_SLOT_HEAD + 1, (HELM >> 32) as u32),
            ],
        );
        crate::items::test_spawn_item(
            app.world_mut(),
            HELM,
            ObjectFields::from_pairs(&[(F_OBJECT_ENTRY, HELM_ID), (22, 1900), (43, 7), (44, 862)]),
            false,
        );
        let link: Option<String> = eval(&mut app, r#"return GetInventoryItemLink("player", 1)"#);
        assert_eq!(
            link.as_deref(),
            Some("|cffffffff|Hitem:7366:1900:862:7|h[]|h|r")
        );
    }

    /// A wrapped item names no random property (`0x5d8bc0`: `ITEM_FIELD_FLAGS & 0x8` passes 0 to
    /// `0x5d8b00`, `0x5d8bc9`-`0x5d8bcf`), so a worn slot's name, the tooltip's title, carries no
    /// suffix of the item inside.
    #[test]
    fn a_wrapped_items_slot_name_hides_the_roll() {
        use benilla_formats::{RandomProperty, RandomPropertyCatalog};
        use benilla_protocol::field::FIELD_OBJECT_ENTRY;
        const GIFT_ID: u32 = 5043;
        /// `ITEM_FIELD_FLAGS`.
        const F_ITEM_FLAGS: u16 = 21;
        /// `ITEM_FIELD_RANDOM_PROPERTIES_ID`.
        const F_ROLL: u16 = 44;
        let props = crate::items::RandomProperties::new(RandomPropertyCatalog::from_rows(
            std::collections::HashMap::from([(
                862,
                RandomProperty {
                    suffix: "of the Bear".into(),
                    enchants: [0; 5],
                },
            )]),
        ));
        let mut items = Items::default();
        items.insert_template(
            GIFT_ID,
            Some(crate::items::test_template("Red Ribboned Gift")),
        );
        let names = crate::names::NameCache::default();
        let mut objs = crate::ui_items::TestObjects::new();
        for (guid, flags) in [(0xC1, 0), (0xC2, 0x8)] {
            objs.spawn(
                guid,
                ObjectFields::from_pairs(&[
                    (2, 0x3),
                    (FIELD_OBJECT_ENTRY, GIFT_ID),
                    (F_ROLL, 862),
                    (F_ITEM_FLAGS, flags),
                ]),
            );
        }
        let objects = objs.get();
        let name = |guid: u64| {
            let rolls = crate::items::RollCatalogs {
                enchants: None,
                props: Some(&props),
            };
            slot_view(&objects, &items, None, rolls, &names, None, false, guid)
                .expect("a view")
                .name
        };
        assert_eq!(
            name(0xC1).as_deref(),
            Some("Red Ribboned Gift of the Bear"),
            "unwrapped, the roll names"
        );
        assert_eq!(name(0xC2).as_deref(), Some("Red Ribboned Gift"));
    }

    /// The ammo asks a harness's VM kept, asked as the host asks them inside the call
    /// ([`benilla_ui::script::RecordAsk::AmmoItem`]).
    fn ask_kept_ammo(app: &mut App) {
        let entries = app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .take_ammo_record_asks();
        for entry in entries {
            ask_ammo_record(
                app.world().resource::<Items>(),
                app.world().resource::<NetCommands>(),
                entry,
                0,
            );
        }
    }

    /// The ammo slot getters' record lookups with the callback `0x4c84f0` (`0x4c83bc`,
    /// `0x4c89cf`): a call that misses the cache asks and waits for the answer, one callback per
    /// call; once the record is cached the call reads it itself and asks nothing. The slot's view
    /// only reads the record, so a getter without the callback sends nothing.
    #[test]
    fn an_ammo_getters_miss_waits_and_a_hit_asks_nothing() {
        const ARROW: u32 = 2512;
        /// `PLAYER_AMMO_ID`.
        const AMMO_ID: u16 = 1223;
        let mut app = world_with_bank_bag(None);
        let (tx, rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        set_self(&mut app, &[(AMMO_ID, ARROW)]);
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .keep_calls();
        let ask = |app: &mut App| {
            let quality: Option<i64> = eval(app, r#"return GetInventoryItemQuality("player", 0)"#);
            ask_kept_ammo(app);
            quality
        };
        let waiters = |app: &mut App| {
            app.world_mut()
                .resource_mut::<Items>()
                .take_ammo_waiters(ARROW)
        };
        let asked = |rx: &crossbeam_channel::Receiver<crate::net::ClientCommand>| {
            crate::query_cache::queries(rx)
                .into_iter()
                .filter(|c| {
                    matches!(c, crate::net::ClientCommand::ItemQuery { entry, .. } if *entry == ARROW)
                })
                .count()
        };
        // The count getter builds the slot's view, which only reads the record: none carried is
        // its 0, where no view is the empty slot's 1 (`0x4c8797`). The link getter answers the
        // slot ahead of the view (`0x4c8c8a`).
        let count: i64 = eval(&mut app, r#"return GetInventoryItemCount("player", 0)"#);
        ask_kept_ammo(&mut app);
        assert_eq!(count, 0, "the view, with none carried");
        assert_eq!(asked(&rx), 0, "the view reads");
        assert_eq!(ask(&mut app), None);
        assert_eq!(ask(&mut app), None);
        assert_eq!(asked(&rx), 1, "one query in flight");
        assert_eq!(waiters(&mut app), 2, "each miss waits for its answer");
        app.world_mut()
            .resource_mut::<Items>()
            .insert_template(ARROW, Some(crate::items::test_template("Rough Arrow")));
        assert!(
            ask(&mut app).is_some(),
            "the landed record, read in the call"
        );
        assert_eq!(waiters(&mut app), 0, "a hit registers no callback");
    }

    /// The lock is the VM's, keyed by the guid the slot holds: `IsInventoryItemLocked` reads it on
    /// the bank bag's own guid, at a banker (`GetItemInSlot`'s bank gate, `0x6228a0`).
    #[test]
    fn a_bank_bag_reads_the_lock_on_its_own_guid() {
        let mut app = world_with_bank_bag(Some(BAG));
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .set_bank(Some(benilla_ui::script::BankState::default()));
        let locked = r#"return IsInventoryItemLocked(64) and true or false"#;
        assert!(!eval::<bool>(&mut app, locked));
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .lock_item(BAG);
        assert!(eval::<bool>(&mut app, locked), "a send's lock on the bag");
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .unlock_item(BAG);
        assert!(!eval::<bool>(&mut app, locked), "the server's answer");
        let seen: Vec<String> = eval(&mut app, "return SEEN");
        assert!(seen.is_empty(), "a lock is no inventory change: {seen:?}");
    }

    /// A bank bag's object reads in the next call, with no push and no event between: its record
    /// landing names its link at once (`0x52ae00`).
    #[test]
    fn a_bank_bag_reads_its_object_at_the_call() {
        let mut app = world_with_bank_bag(Some(BAG));
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .set_bank(Some(benilla_ui::script::BankState::default()));
        let link = r#"return GetInventoryItemLink("player", 64)"#;
        assert!(eval::<String>(&mut app, link).contains("[Traveler's Backpack]"));
        app.world_mut()
            .resource_mut::<Items>()
            .insert_template(BAG_ENTRY, Some(crate::items::test_template("Mule Pack")));
        assert!(eval::<String>(&mut app, link).contains("[Mule Pack]"));
        let seen: Vec<String> = eval(&mut app, "return SEEN");
        assert!(seen.is_empty(), "{seen:?}");
    }

    // The UNIT-block field indices the core reads (`benilla_protocol`'s `fields::FIELD_UNIT_*`).
    const BASEATTACKTIME: u16 = 126;
    const MINDAMAGE: u16 = 134;
    const MAXDAMAGE: u16 = 135;
    const STAT0: u16 = 150;
    const RESISTANCES0: u16 = 155;
    const ATTACK_POWER: u16 = 165;
    const ATTACK_POWER_MODS: u16 = 166;

    /// A boar's descriptor: the UNIT block alone, built as a create block, because a live pet
    /// arrives as one and a create reads an absent field as 0, not unknown.
    fn boar() -> ObjectStore {
        ObjectStore(
            ObjectFields::from_pairs(&[
                (STAT0, 63),     // strength
                (STAT0 + 1, 45), // agility
                (STAT0 + 2, 68), // stamina
                (STAT0 + 3, 32), // intellect
                (STAT0 + 4, 42), // spirit
                (RESISTANCES0, 1810),
                (RESISTANCES0 + 2, 15), // fire
                (BASEATTACKTIME, 2000),
                (MINDAMAGE, 30.5f32.to_bits()),
                (MAXDAMAGE, 44.5f32.to_bits()),
                (ATTACK_POWER, 178),
                // The MODS field is a packed signed pair: pos in the low word, neg in the high.
                (ATTACK_POWER_MODS, (((-4i16) as u16 as u32) << 16) | 12),
            ])
            .into_created(benilla_protocol::messages::ObjectType::Unit),
        )
    }

    /// PLAYER-block values keep their defaults: a creature has no buff split.
    #[test]
    fn the_core_reads_a_creatures_unit_block_and_defaults_the_rest() {
        let s = unit_combat_stats(&boar());
        assert_eq!(s.stats, [63, 45, 68, 32, 42]);
        assert_eq!(s.resistances, [1810, 0, 15, 0, 0, 0, 0]);
        assert_eq!(s.min_damage, 30.5);
        assert_eq!(s.max_damage, 44.5);
        assert_eq!(s.main_attack_time_ms, 2000);
        assert_eq!(
            (s.attack_power, s.attack_power_pos, s.attack_power_neg),
            (178, 12, -4)
        );
        assert_eq!(s.stat_pos, [0; 5]);
        assert_eq!(s.stat_neg, [0; 5]);
        assert_eq!(s.resistance_pos, [0; 7]);
        assert_eq!(s.resistance_neg, [0; 7]);
        assert_eq!((s.physical_bonus_pos, s.physical_bonus_neg), (0, 0));
        assert_eq!(s.damage_percent, 1.0);
        assert!(!s.has_offhand && !s.has_wand);
        assert_eq!(s.main_weapon_skill, (0, 0));
        assert_eq!(s.ranged_weapon_skill, (0, 0));
        assert_eq!(
            s.defense_skill,
            (0, 0),
            "a creature has no defense pair; `UnitDefense` answers its level × 5 itself"
        );
    }

    /// `UnitAttackSpeed` reads `UNIT_FIELD_BASEATTACKTIME` as it stands (`0x518ea0`), so a unit
    /// whose field is unsent answers 0, with no stand-in speed.
    #[test]
    fn an_unstreamed_unit_reads_the_absent_shape() {
        let store = ObjectStore(ObjectFields::from_pairs(&[]));
        let s = unit_combat_stats(&store);
        assert_eq!(s.stats, [0; 5]);
        assert_eq!(s.resistances, [0; 7]);
        assert_eq!(s.damage_percent, 1.0);
        assert_eq!(
            (
                s.main_attack_time_ms,
                s.offhand_attack_time_ms,
                s.ranged_attack_time_ms
            ),
            (0, 0, 0)
        );
        const PET: u64 = 0xF140_0000_0000_0077;
        let pet =
            crate::ui_unit::snapshot(&store, PET, None, 0, Default::default(), Default::default());
        let mut s = UiScript::new().unwrap();
        s.set_unit("pet", Some(pet));
        assert_eq!(
            s.eval::<f64>(r#"return (UnitAttackSpeed("pet"))"#).unwrap(),
            0.0
        );
    }

    /// `UnitAttackPower` and `UnitRangedAttackPower` scale each of their three values by
    /// `1 + multiplier` (fields 167 and 170) and round each alone: the product is stored as a
    /// single and `fistp`'d to nearest even (`0x518fe2`-`0x518ffd`), so 1.5 × 3 is 4, not 5.
    #[test]
    fn each_attack_power_value_is_scaled_by_its_multiplier_and_rounded_alone() {
        const PET: u64 = 0xF140_0000_0000_0077;
        const MULTIPLIER: u16 = 167;
        const RANGED: u16 = 168;
        let pair = |pos: i16, neg: i16| (u32::from(neg as u16) << 16) | u32::from(pos as u16);
        let mut store = boar();
        store.0.merge(ObjectFields::from_pairs(&[
            (ATTACK_POWER, 3),
            (ATTACK_POWER_MODS, pair(5, -1)),
            (MULTIPLIER, 0.5f32.to_bits()),
            (RANGED, 7),
            (RANGED + 1, pair(1, -3)),
            (RANGED + 2, 0.5f32.to_bits()),
        ]));
        let pet =
            crate::ui_unit::snapshot(&store, PET, None, 0, Default::default(), Default::default());
        let mut s = UiScript::new().unwrap();
        s.set_unit("pet", Some(pet));
        assert_eq!(
            s.eval::<(i64, i64, i64)>(r#"return UnitAttackPower("pet")"#)
                .unwrap(),
            (4, 8, -2),
            "4.5, 7.5 and -1.5"
        );
        assert_eq!(
            s.eval::<(i64, i64, i64)>(r#"return UnitRangedAttackPower("pet")"#)
                .unwrap(),
            (10, 2, -4),
            "10.5, 1.5 and -4.5"
        );
    }

    /// `GetInventoryItemQuality` answers nil while the slot's record is not cached (`0x4c8a11`),
    /// and the record's quality only when its inventory type is nonzero, else -1
    /// (`0x4c8a13`-`0x4c8a22`).
    #[test]
    fn the_quality_getter_is_nil_uncached_and_minus_one_for_inventory_type_zero() {
        use benilla_protocol::field::FIELD_PLAYER_INV_SLOT_HEAD;
        const HELM: u64 = 0x4000_0000_0000_0b02;
        const HELM_ID: u32 = 7366;
        let mut app = world_with_bank_bag(None);
        set_self(
            &mut app,
            &[
                (FIELD_PLAYER_INV_SLOT_HEAD, HELM as u32),
                (FIELD_PLAYER_INV_SLOT_HEAD + 1, (HELM >> 32) as u32),
            ],
        );
        crate::items::test_spawn_item(
            app.world_mut(),
            HELM,
            ObjectFields::from_pairs(&[(F_OBJECT_ENTRY, HELM_ID)]),
            false,
        );
        let quality = |app: &mut App| {
            eval::<Option<i64>>(app, r#"return GetInventoryItemQuality("player", 1)"#)
        };
        assert_eq!(quality(&mut app), None, "the record is not cached");
        let helm = crate::items::test_template("Helm");
        app.world_mut()
            .resource_mut::<Items>()
            .insert_template(HELM_ID, Some(helm.clone()));
        assert_eq!(quality(&mut app), Some(-1), "inventory type 0");
        app.world_mut().resource_mut::<Items>().insert_template(
            HELM_ID,
            Some(benilla_protocol::ItemInfo {
                inventory_type: 1,
                quality: 3,
                ..helm
            }),
        );
        assert_eq!(quality(&mut app), Some(3));
    }

    /// A held unit's snapshot carries its own fields, so the pet sheet's reads answer through the
    /// `"pet"` token as any other token's do (`PetPaperDollFrame.lua:112`, `:149`), and a unit we
    /// summoned answers `UnitDefense` with its level times 5 (`0x519272`, `0x613680`).
    #[test]
    fn a_held_units_snapshot_answers_the_stat_bindings() {
        const ME: u64 = 0x10;
        const PET: u64 = 0xF140_0000_0000_0077;
        /// `UNIT_FIELD_SUMMONEDBY` and `UNIT_FIELD_LEVEL`.
        const SUMMONEDBY: u16 = 12;
        const LEVEL: u16 = 34;
        let mut store = boar();
        store.0.merge(ObjectFields::from_pairs(&[
            (SUMMONEDBY, ME as u32),
            (LEVEL, 40),
        ]));
        let pet =
            crate::ui_unit::snapshot(&store, PET, None, 0, Default::default(), Default::default());
        let mut s = UiScript::new().unwrap();
        s.set_unit_guids(&benilla_ui::script::UnitGuids {
            player: ME,
            pet: PET,
            ..Default::default()
        });
        s.set_unit("pet", Some(pet));
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(r#"return UnitStat("pet", 3)"#)
                .unwrap(),
            (68, 68, 0, 0)
        );
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(r#"return UnitResistance("pet", 2)"#)
                .unwrap(),
            (15, 15, 0, 0)
        );
        assert_eq!(
            s.eval::<(i64, i64, i64, i64, i64)>(r#"return UnitArmor("pet")"#)
                .unwrap(),
            (1810, 1810, 1810, 0, 0)
        );
        assert_eq!(
            s.eval::<(f64, f64, f64, f64, i64, i64, f64)>(r#"return UnitDamage("pet")"#)
                .unwrap(),
            (30.5, 44.5, 0.0, 0.0, 0, 0, 1.0)
        );
        assert_eq!(
            s.eval::<(i64, i64, i64)>(r#"return UnitAttackPower("pet")"#)
                .unwrap(),
            (178, 12, -4)
        );
        assert_eq!(
            s.eval::<(i64, i64)>(r#"return UnitDefense("pet")"#)
                .unwrap(),
            (200, 0)
        );
        s.set_unit("pet", None);
        assert_eq!(
            s.eval::<(i64, i64, i64, i64, i64)>(r#"return UnitArmor("pet")"#)
                .unwrap(),
            (0, 0, 0, 0, 0)
        );
    }

    /// `UnitRangedDamage`'s bonus is read at the ranged weapon's school (`0x5189e1`-`0x518a45`):
    /// the first damage line of the ranged slot's record with a minimum above 0, its school at
    /// `+0xe0`; school 0 with no such line or no record.
    #[test]
    fn the_ranged_bonus_is_read_at_the_ranged_weapons_school() {
        use benilla_protocol::field::{FIELD_OBJECT_ENTRY, FIELD_PLAYER_INV_SLOT_HEAD};
        /// `PLAYER_FIELD_MOD_DAMAGE_DONE_POS`, `_NEG` and `_PCT`, seven schools each.
        const POS: u16 = 1201;
        const NEG: u16 = 1208;
        const PCT: u16 = 1215;
        const WAND: u64 = 0x4000_0000_0000_0077;
        const WAND_ENTRY: u32 = 5240;
        let ranged = FIELD_PLAYER_INV_SLOT_HEAD + 2 * u16::from(SLOT_RANGED);
        let store = ObjectStore(ObjectFields::from_pairs(&[
            (ranged, WAND as u32),
            (ranged + 1, (WAND >> 32) as u32),
            (POS, 3),
            (POS + 5, 20),
            (NEG + 5, (-4i32) as u32),
            (PCT, 1.0f32.to_bits()),
            (PCT + 5, 1.15f32.to_bits()),
        ]));
        let mut objs = crate::ui_items::TestObjects::new();
        objs.spawn(
            WAND,
            ObjectFields::from_pairs(&[(2, 0x3), (FIELD_OBJECT_ENTRY, WAND_ENTRY)]),
        );
        let mut items = Items::default();
        let wand = |damages: Vec<benilla_protocol::messages::ItemDamage>| {
            Some(benilla_protocol::messages::ItemInfo {
                class: ITEM_CLASS_WEAPON,
                subclass: SUBCLASS_WAND,
                damages,
                ..crate::items::test_template("Wand")
            })
        };
        let shadow = |min: f32| benilla_protocol::messages::ItemDamage {
            min,
            max: 30.0,
            school: 5,
        };
        // A line with a minimum of 0 is passed over for the next.
        items.insert_template(
            WAND_ENTRY,
            wand(vec![
                benilla_protocol::messages::ItemDamage {
                    min: 0.0,
                    max: 10.0,
                    school: 2,
                },
                shadow(20.0),
            ]),
        );
        let s = combat_stats(&store, &objs.get(), &items);
        assert_eq!(
            (s.ranged_bonus_pos, s.ranged_bonus_neg, s.ranged_percent),
            (20, -4, 1.15)
        );
        assert_eq!(s.physical_bonus_pos, 3, "the melee bonus stays physical");
        // No line above 0: school 0.
        items.insert_template(WAND_ENTRY, wand(vec![shadow(0.0)]));
        let s = combat_stats(&store, &objs.get(), &items);
        assert_eq!(
            (s.ranged_bonus_pos, s.ranged_bonus_neg, s.ranged_percent),
            (3, 0, 1.0)
        );
    }
}

#[cfg(test)]
#[path = "ui_char_call_tests.rs"]
mod call_tests;
