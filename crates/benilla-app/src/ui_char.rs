//! The character-window feed: the [`UnitCombatStats`] and `InventorySlots` snapshots the paper
//! doll's Lua bindings read, built from our avatar's descriptor and item stores, pushed ahead of
//! the stat events that repaint it ([`crate::ui_unit::field_events`]); the inventory events; and
//! the booth's yaw from the stock `CharacterModelFrame`. [`unit_combat_stats`] is
//! the half any unit shares: a creature has no player fields, so those read absent and keep their
//! defaults.

use bevy::prelude::*;

use benilla_protocol::messages::PLAYER_SKILL_SLOTS;
use benilla_ui::script::{
    weapon_subclass_skill, BankBagSlots, InvSlotView, InventorySlots, ScriptValue, SkillEntry,
    SkillsState, UiScript, UnitCombatStats, BANK_BAG_SLOT_COUNT, SKILL_DEFENSE, SKILL_UNARMED,
};
use benilla_ui::strings::Arg;

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::{ClientCommand, NetCommands, ObjectStore, Objects, SelfPlayer};
use crate::query_cache::Ask;
use crate::ui_items::{find_equip_slot, item_link};
use crate::ui_script::{gate, UiFeed};

/// Equipment slots, 0-based (`EQUIPMENT_SLOT_*`): main hand, off hand, ranged.
const SLOT_MAIN_HAND: u8 = 15;
const SLOT_OFF_HAND: u8 = 16;
const SLOT_RANGED: u8 = 17;
/// Item class 2, weapon (vmangos `ItemPrototype.h:139`).
const ITEM_CLASS_WEAPON: u32 = 2;
/// Weapon subclass 19, wand (`ItemPrototype.h:211`), for `HasWandEquipped`.
const SUBCLASS_WAND: u32 = 19;

/// The last snapshots pushed, so a push happens on change only; held per VM
/// ([`crate::ui_script::VmMemo`]), so a `/reload` re-pushes. The stat events are field watches
/// ([`crate::field_watch`]), never fired off these diffs.
#[derive(Resource, Default)]
pub(crate) struct CharFeedState {
    vm: crate::ui_script::VmMemo<CharFeedMemo>,
}

/// The per-VM half of [`CharFeedState`]: the transition-diff bases.
#[derive(Default)]
struct CharFeedMemo {
    last_stats: Option<UnitCombatStats>,
    last_inv: Option<InventorySlots>,
    last_bank_bags: Option<BankBagSlots>,
    /// The bag guid in each bank bag slot, `PLAYERBANKSLOTS_CHANGED`'s discriminator: the pushed
    /// view cannot tell two copies of one bag apart, and the reference fires when they swap.
    last_bank_bag_guids: [u64; BANK_BAG_SLOT_COUNT],
    /// Counters for the stores whose lazy resolves poison `is_changed` (item templates, creator
    /// names); item objects are entities, watched through [`crate::items::ItemChanges`].
    items_templates: gate::Watch,
    names_generation: gate::Watch,
    /// `ItemChanges::countdown_steps`: one step per displayed-second change, the final elapse too.
    enchant_deadlines: gate::Watch,
}

/// The character-window feed; its bindings are `benilla-ui`'s `char_stats`.
pub(crate) struct UiCharPlugin;

impl Plugin for UiCharPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CharFeedState>()
            .add_systems(
                Startup,
                load_skill_tiers.after(benilla_assets::AssetSet::Open),
            )
            .add_systems(
                Update,
                (
                    // Before `feed_containers`: a `BAG_UPDATE` handler may read the bag item's own
                    // row (`GetInventoryItemLink("player", ContainerIDToInventoryID(bag))`), so the
                    // inventory push lands first.
                    feed_char
                        .before(crate::ui_items::feed::feed_containers)
                        .in_set(UiFeed)
                        .in_set(crate::ui_unit::field_events::FieldEventPushes),
                    watch_skill_ups.in_set(UiFeed),
                    feed_skills.in_set(UiFeed),
                    drain_skill_abandons.in_set(crate::ui_script::UiDrain),
                ),
            );
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

/// The skill block [`watch_skill_ups`] last announced: the self guid and its `skillId → value`
/// map, the diff base for the chat lines and the event.
type SkillBlock = Option<(u64, std::collections::HashMap<u16, u16>)>;

/// One skill-up chat line, `ERR_SKILL_UP_SI` or `ERR_SKILL_GAINED_S`, pushed as `CHAT_MSG_SKILL`.
/// The server sends no skill-up message; the reference's watcher on the skill value words
/// (`0x5de180`) prints these through `0x496720`, silent for a line whose `SkillRaceClassInfo`
/// flags carry `0x402` or that no row admits for this race and class (`0x5de352`), so a level-up
/// prints none. Not `ui_action::show_messages`: that sink sends every chat row as
/// `CHAT_MSG_SYSTEM`, and these two ask for `CHAT_MSG_SKILL` (23).
fn skill_line(script: &UiScript, chat: &crate::ui_chat::ChatLog, key: &str, args: &[Arg<'_>]) {
    let Some(template) = script
        .lua()
        .globals()
        .get::<String>(key)
        .ok()
        .filter(|t| !t.is_empty())
    else {
        return;
    };
    chat.push_event(crate::ui_chat::ChatEvent::text_only(
        crate::ui_chat::ChatEventKind::Skill,
        benilla_ui::strings::fill(&template, args),
    ));
}

fn watch_skill_ups(
    mut hosted_vm: crate::script_calls::HostedVm,
    self_guid: Res<crate::net::SelfGuid>,
    self_store: Query<&ObjectStore, With<SelfPlayer>>,
    skill_lines: Option<Res<crate::ui_spellbook::SkillLines>>,
    chat: Res<crate::ui_chat::ChatLog>,
    mut prev: Local<crate::ui_script::VmMemo<SkillBlock>>,
) {
    hosted_vm.run(|script| {
        let (Some(guid), Ok(store)) = (self_guid.0, self_store.single()) else {
            return;
        };
        // The memo is per VM and a new VM re-seeds silently; with no VM nothing is diffed, or a
        // stale skill-up would replay at the next login. `SKILL_LINES_CHANGED` fires here only when
        // a line is added, removed or changes value; the reference fires it at the end of every
        // skill-list rebuild (`0x4d2fec`), which its value, max, permanent-bonus and
        // temporary-bonus watchers run (registered `0x5dda54`-`0x5ddaf0`), and from both header
        // collapse and expand verbs.
        let Some(script) = script else {
            return;
        };
        let prev = prev.get(script);
        let mut cur = std::collections::HashMap::new();
        for i in 0..PLAYER_SKILL_SLOTS {
            if let Some(s) = store.0.player_skill(i) {
                if s.skill_id != 0 {
                    cur.insert(s.skill_id, s.value);
                }
            }
        }
        match prev.as_ref() {
            // First fill: seed silently, no lines; the event fires once.
            None => {
                if cur.is_empty() {
                    return;
                }
                script.fire_event("SKILL_LINES_CHANGED", vec![]);
                *prev = Some((guid, cur));
            }
            Some((prev_guid, _)) if *prev_guid != guid => {
                // A different character: re-seed, never diff across characters.
                script.fire_event("SKILL_LINES_CHANGED", vec![]);
                *prev = Some((guid, cur));
            }
            Some((_, prev_map)) => {
                if *prev_map == cur {
                    return;
                }
                // The `0x402` gate reads the player's own race and class row, as `skills_row` does.
                let (race, class) = (
                    store.0.unit_race().unwrap_or(0),
                    store.0.unit_class().unwrap_or(0),
                );
                for (&id, &value) in &cur {
                    let line_id = u32::from(id);
                    let name = || {
                        skill_lines
                            .as_ref()
                            .and_then(|s| s.catalog.line(line_id))
                            .map(|l| l.name.clone())
                    };
                    let announces = || {
                        skill_lines
                            .as_ref()
                            .is_some_and(|s| s.catalog.announces_skill_ups(line_id, race, class))
                    };
                    match prev_map.get(&id) {
                        // A rank-up: `ERR_SKILL_UP_SI` (catalog row 54), not `SKILL_RANK_UP`, the
                        // same enUS sentence with no catalog row.
                        Some(&old) if value > old => {
                            if let Some(name) = name() {
                                if announces() {
                                    debug!("chat: skill-up announced ({name} {old}→{value})");
                                    skill_line(
                                        script,
                                        &chat,
                                        "ERR_SKILL_UP_SI",
                                        &[Arg::S(&name), Arg::D(i64::from(value))],
                                    );
                                } else {
                                    debug!(
                                        "chat: skill-up silenced ({name} {old}→{value}, the \
                                            0x402 gate)"
                                    );
                                }
                            }
                        }
                        // A new line: `ERR_SKILL_GAINED_S` (catalog row 53).
                        None => {
                            if let Some(name) = name() {
                                if announces() {
                                    debug!("chat: skill-gain announced ({name} at {value})");
                                    skill_line(
                                        script,
                                        &chat,
                                        "ERR_SKILL_GAINED_S",
                                        &[Arg::S(&name)],
                                    );
                                } else {
                                    debug!(
                                        "chat: skill-gain silenced ({name} at {value}, the 0x402 \
                                         gate)"
                                    );
                                }
                            }
                        }
                        _ => {}
                    }
                }
                script.fire_event("SKILL_LINES_CHANGED", vec![]);
                *prev = Some((guid, cur));
            }
        }
    })
}

/// The skills-pane feed: every `PLAYER_SKILL_INFO` line the reference lists, as a flat
/// [`SkillEntry`] pushed through `set_skills` on change; the engine groups, sorts and folds.
fn feed_skills(
    mut hosted_vm: crate::script_calls::HostedVm,
    self_store: Query<Ref<ObjectStore>, With<SelfPlayer>>,
    skill_lines: Option<Res<crate::ui_spellbook::SkillLines>>,
    tiers: Option<Res<SkillTiersRes>>,
    mut last: Local<crate::ui_script::VmMemo<Option<SkillsState>>>,
    mut tiers_pushed: Local<crate::ui_script::VmMemo<bool>>,
) {
    hosted_vm.run(|script| {
        let Some(script) = script else {
            return;
        };
        // The cost table, once per VM.
        let pushed = tiers_pushed.get(script);
        if !*pushed {
            if let Some(tiers) = tiers.as_deref() {
                script.set_skill_tiers(tiers.0.tiers());
                *pushed = true;
            }
        }
        let last = last.get(script);
        let lines_moved = skill_lines.as_ref().is_some_and(|l| l.is_changed());
        let (Ok(store), Some(skill_lines)) = (self_store.single(), skill_lines.as_deref()) else {
            return;
        };
        // A pure function of the descriptor and the catalog: with both still, nothing can differ.
        if last.is_some() && !store.is_changed() && !lines_moved {
            return;
        }
        let (race, class) = (
            store.0.unit_race().unwrap_or(0),
            store.0.unit_class().unwrap_or(0),
        );
        let level = store.0.unit_level().unwrap_or(0);
        let mut entries = Vec::new();
        for i in 0..PLAYER_SKILL_SLOTS {
            let Some(slot) = store.0.player_skill(i) else {
                continue;
            };
            if let Some(e) = skills_row(&slot, &skill_lines.catalog, race, class, level) {
                entries.push(e);
            }
        }
        let fresh = SkillsState { entries };
        if last.as_ref() == Some(&fresh) {
            return;
        }
        script.set_skills(fresh.clone());
        *last = Some(fresh);
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

/// Each queued `AbandonSkill` becomes one `CMSG_UNLEARN_SKILL`; nothing changes locally, as the
/// server's `SetSkill(id, 0, 0)` (vmangos `SkillHandler.cpp:69`) returns as a skill-field update.
fn drain_skill_abandons(mut hosted_vm: crate::script_calls::HostedVm, commands: Res<NetCommands>) {
    hosted_vm.run(|script| {
        let Some(script) = script else {
            return;
        };
        for skill_id in script.take_skill_abandons() {
            debug!("ui_char: abandon skill line {skill_id}");
            let _ = commands.0.send(ClientCommand::UnlearnSkill { skill_id });
        }
    })
}

/// The item entry in an equipped slot; `None` for an empty slot or an item not yet streamed.
fn slot_entry(store: &ObjectStore, objects: &Objects, slot0: u8) -> Option<u32> {
    let guid = store.0.player_inv_slot(slot0)?;
    objects.object(guid)?.object_entry()
}

/// The weapon-skill line for `slot0`: vmangos's subclass table (`Item.cpp:700`) for a weapon,
/// Unarmed (162) for anything else.
fn weapon_skill_id(store: &ObjectStore, objects: &Objects, items: &Items, slot0: u8) -> u32 {
    let Some(entry) = slot_entry(store, objects, slot0) else {
        return SKILL_UNARMED;
    };
    // A worn item's record only read: its create asked.
    let Some(t) = items.template(entry, 0, Ask::Read) else {
        return SKILL_UNARMED; // in flight: refined when the template lands
    };
    if t.class != ITEM_CLASS_WEAPON {
        return SKILL_UNARMED;
    }
    weapon_subclass_skill(t.subclass).unwrap_or(SKILL_UNARMED)
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

pub(crate) fn unit_combat_stats(store: &ObjectStore) -> UnitCombatStats {
    let f = &store.0;

    let mut stats = [0i32; 5];
    let mut stat_pos = [0i32; 5];
    let mut stat_neg = [0i32; 5];
    for i in 0..5u8 {
        stats[usize::from(i)] = f.unit_stat(i).unwrap_or(0) as i32;
        // The buff-split arrays are integers on the wire, not floats.
        stat_pos[usize::from(i)] = f.player_posstat(i).unwrap_or(0);
        stat_neg[usize::from(i)] = f.player_negstat(i).unwrap_or(0);
    }
    let mut resistances = [0i32; 7];
    let mut resistance_pos = [0i32; 7];
    let mut resistance_neg = [0i32; 7];
    for i in 0..7u8 {
        resistances[usize::from(i)] = f.unit_resistance(i).unwrap_or(0);
        resistance_pos[usize::from(i)] = f.player_resistance_buff_pos(i).unwrap_or(0);
        resistance_neg[usize::from(i)] = f.player_resistance_buff_neg(i).unwrap_or(0);
    }

    let (ap_pos, ap_neg) = f.unit_attack_power_mods();
    let (rap_pos, rap_neg) = f.unit_ranged_attack_power_mods();

    UnitCombatStats {
        stats,
        stat_pos,
        stat_neg,
        resistances,
        resistance_pos,
        resistance_neg,
        min_damage: f.unit_min_damage().unwrap_or(0.0),
        max_damage: f.unit_max_damage().unwrap_or(0.0),
        min_offhand_damage: f.unit_min_offhand_damage().unwrap_or(0.0),
        max_offhand_damage: f.unit_max_offhand_damage().unwrap_or(0.0),
        physical_bonus_pos: f.player_mod_damage_done_pos(0).unwrap_or(0),
        physical_bonus_neg: f.player_mod_damage_done_neg(0).unwrap_or(0),
        damage_percent: f.player_mod_damage_done_pct(0).unwrap_or(1.0),
        // School 0 here; the player's ranged school is `combat_stats`' to read.
        ranged_bonus_pos: f.player_mod_damage_done_pos(0).unwrap_or(0),
        ranged_bonus_neg: f.player_mod_damage_done_neg(0).unwrap_or(0),
        ranged_percent: f.player_mod_damage_done_pct(0).unwrap_or(1.0),
        // As the fields stand, 0 when unsent (`0x518ea0`).
        main_attack_time_ms: f.unit_base_attack_time(0).unwrap_or(0),
        offhand_attack_time_ms: f.unit_base_attack_time(1).unwrap_or(0),
        attack_power: f.unit_attack_power().unwrap_or(0),
        attack_power_pos: i32::from(ap_pos),
        attack_power_neg: i32::from(ap_neg),
        attack_power_multiplier: f.unit_attack_power_multiplier().unwrap_or(0.0),
        ranged_attack_power: f.unit_ranged_attack_power().unwrap_or(0),
        ranged_attack_power_pos: i32::from(rap_pos),
        ranged_attack_power_neg: i32::from(rap_neg),
        ranged_attack_power_multiplier: f.unit_ranged_attack_power_multiplier().unwrap_or(0.0),
        ranged_attack_time_ms: f.unit_ranged_attack_time().unwrap_or(0),
        ranged_min_damage: f.unit_min_ranged_damage().unwrap_or(0.0),
        ranged_max_damage: f.unit_max_ranged_damage().unwrap_or(0.0),
        // The equipment and skill half is `combat_stats`'s; any other unit keeps these defaults.
        has_offhand: false,
        has_wand: false,
        main_weapon_skill: (0, 0),
        offhand_weapon_skill: (0, 0),
        ranged_weapon_skill: (0, 0),
        defense_skill: (0, 0),
        dodge_percent: 0.0,
        parry_percent: 0.0,
        block_percent: 0.0,
    }
}

/// The player's snapshot: [`unit_combat_stats`] plus the equipment flags and the skill pairs.
fn combat_stats(store: &ObjectStore, objects: &Objects, items: &Items) -> UnitCombatStats {
    // The off hand counts only as a weapon (a shield does not swing); both flags read false
    // until the template lands.
    let has_offhand = slot_entry(store, objects, SLOT_OFF_HAND)
        .and_then(|e| items.template(e, 0, Ask::Read))
        .is_some_and(|t| t.class == ITEM_CLASS_WEAPON);
    let has_wand = slot_entry(store, objects, SLOT_RANGED)
        .and_then(|e| items.template(e, 0, Ask::Read))
        .is_some_and(|t| t.class == ITEM_CLASS_WEAPON && t.subclass == SUBCLASS_WAND);

    let main_skill = weapon_skill_id(store, objects, items, SLOT_MAIN_HAND);
    let offhand_skill = weapon_skill_id(store, objects, items, SLOT_OFF_HAND);
    let ranged_skill_id = slot_entry(store, objects, SLOT_RANGED)
        .and_then(|e| items.template(e, 0, Ask::Read))
        .filter(|t| t.class == ITEM_CLASS_WEAPON)
        .and_then(|t| weapon_subclass_skill(t.subclass));
    // `UnitRangedDamage`'s school (`0x5189e1`-`0x518a45`): the ranged-slot item's record, read
    // without asking, and its first damage line whose minimum is above 0, else school 0. The
    // template keeps only lines whose maximum is above 0, which drops no well-formed such line.
    let ranged_school = slot_entry(store, objects, SLOT_RANGED)
        .and_then(|e| items.template(e, 0, Ask::Read))
        .and_then(|t| t.damages.iter().find(|d| d.min > 0.0))
        .map_or(0, |d| u8::try_from(d.school).unwrap_or(u8::MAX));
    let f = &store.0;

    UnitCombatStats {
        has_offhand,
        has_wand,
        main_weapon_skill: skill_pair(store, main_skill),
        // `UnitAttackBothHands`'s second pair; a shield or an empty off hand reads Unarmed.
        offhand_weapon_skill: skill_pair(store, offhand_skill),
        ranged_weapon_skill: ranged_skill_id.map_or((0, 0), |id| skill_pair(store, id)),
        // `UnitDefense`'s pair. The 1.12 player sheet has no Defense row: its `SKILL_LINES_CHANGED`
        // arm (`PaperDollFrame.lua:70-72`) redraws only the Attack row.
        defense_skill: skill_pair(store, SKILL_DEFENSE),
        // `GetDodgeChance`/`GetParryChance`/`GetBlockChance`: player fields, already percentages.
        dodge_percent: store.0.player_dodge_percentage().unwrap_or(0.0),
        parry_percent: store.0.player_parry_percentage().unwrap_or(0.0),
        block_percent: store.0.player_block_percentage().unwrap_or(0.0),
        // The three at that school (`0x518a77`, `0x518ac0`, `0x518b0e`).
        ranged_bonus_pos: f.player_mod_damage_done_pos(ranged_school).unwrap_or(0),
        ranged_bonus_neg: f.player_mod_damage_done_neg(ranged_school).unwrap_or(0),
        ranged_percent: f.player_mod_damage_done_pct(ranged_school).unwrap_or(1.0),
        ..unit_combat_stats(store)
    }
}

/// One slot's view from the item object and its ask-once template (no icon while in flight).
/// Keyed by guid and live id: the doll (`PLAYER_FIELD_INV_SLOT_*`, live id = slot + 1) and the
/// bank bags (`PLAYER_FIELD_BANK_BAG_SLOT_*`, 64..=69) read their guids from different arrays.
fn slot_view(
    objects: &Objects,
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
    let countdowns = objects.countdowns(guid);
    let enchant_ms: [Option<u64>; crate::items::ENCHANT_SLOTS] =
        std::array::from_fn(|s| countdowns.and_then(|c| c.enchant_remaining_display_ms(s as u32)));
    let duration_ms = countdowns.and_then(|c| c.lifetime_remaining_display_ms());
    let obj = objects.object(guid)?;
    let entry = obj.object_entry()?;
    let count = obj.item_stack_count().unwrap_or(1).max(1);
    // `OBJECT_FIELD_TYPE & TYPEMASK_CONTAINER`, read off the object as `GetInventoryItemCount`'s
    // first fork reads it (`0x4c87a6`).
    let is_container = obj.is_container();
    let content_guids: Vec<u64> = {
        let slots = obj.container_num_slots().unwrap_or(0).min(36) as u8;
        (0..slots).filter_map(|s| obj.container_slot(s)).collect()
    };
    let durability = obj
        .item_durability()
        .zip(obj.item_max_durability())
        .filter(|&(_, max)| max > 0);
    // `ITEM_FIELD_FLAGS`, for its wrapped (0x08) and force-red (0x10) bits.
    let flags = obj.item_flags().unwrap_or(0);
    // Soulbound or carrying a binding enchant (`0x5da2c0`), read off the raw descriptor, never off
    // the enchant lines below.
    let already_bound = crate::items::already_bound(obj, rolls.enchants);
    // Only read: the tooltip build asks the creator (`crate::ui_items::feed::ask_item_build_lookups`).
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
    // `ITEM_FIELD_RANDOM_PROPERTIES_ID`, the roll behind an "of the Bear" name; its enchants are
    // already in the slots above.
    let roll = obj.item_random_properties_id();
    // Only read, as the inventory getters read it (`0x4c8a0a`, `0x4c8d34`): the item's create asked.
    let t = items.template(entry, guid, Ask::Read);
    let (name, quality, display, link, equip_slots, kind, bar_placeable, consumable) = match t {
        Some(t) => {
            let name = rolls.name(&t.name, roll);
            (
                Some(name.clone()),
                t.quality as i32,
                t.display_info_id,
                Some(crate::ui_items::item_link_full(
                    entry, 0, roll, 0, &name, t.quality,
                )),
                find_equip_slot(t.inventory_type, has_relic_slot),
                (t.class, t.subclass),
                t.placeable_on_action_bar(),
                t.is_consumable(),
            )
        }
        None => (None, 0, 0, None, Vec::new(), (0, 0), false, false),
    };
    // A container counts its contents' stacks when its subclass's `DisplayFlags` has 0x4, else 0
    // (`GetInventoryItemCount` `0x4c8680`: `0x4c881a`, summed by `0x622130`); the bit is on Soul
    // Bag (1/1) and the Quiver class (11/0..3) only, so a plain bag shows no count.
    let counts_contents = sub_classes.is_some_and(|c| c.0.display_flags(kind.0, kind.1) & 0x4 != 0);
    let contents_count = is_container.then(|| {
        if counts_contents {
            content_guids
                .iter()
                .filter_map(|&g| objects.object(g))
                .map(|o| o.item_stack_count().unwrap_or(1).max(1))
                .sum()
        } else {
            0
        }
    });
    let icon = icons
        .and_then(|i| i.catalog.get(display))
        .and_then(|d| d.icon.clone());
    Some(InvSlotView {
        inventory_type: t.map(|t| t.inventory_type),
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
        icon,
        count,
        contents_count,
        quality,
        name,
        link,
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
    })
}

/// Carried ammo matching `ammo_id`, in the backpack and the four equipped bags; bank and buyback
/// never count.
fn ammo_count(store: &ObjectStore, objects: &Objects, ammo_id: u32) -> u32 {
    let mut n = 0;
    let mut add = |guid: Option<u64>| {
        if let Some(o) = guid.and_then(|g| objects.object(g)) {
            if o.object_entry() == Some(ammo_id) {
                n += o.item_stack_count().unwrap_or(1);
            }
        }
    };
    for i in 0..16u8 {
        add(store.0.player_pack_slot(i));
    }
    for b in 19..23u8 {
        if let Some(bag) = store.0.player_inv_slot(b).and_then(|g| objects.object(g)) {
            let slots = bag.container_num_slots().unwrap_or(0).min(36) as u8;
            for s in 0..slots {
                if let Some(o) = bag.container_slot(s).and_then(|g| objects.object(g)) {
                    if o.object_entry() == Some(ammo_id) {
                        n += o.item_stack_count().unwrap_or(1);
                    }
                }
            }
        }
    }
    n
}

/// The 24-wide inventory snapshot by `GetInventorySlotInfo` id: 0 ammo, 1..=19 the equipment,
/// 20..=23 the equipped bags themselves (inv slots 19..22).
fn inventory_slots(
    store: &ObjectStore,
    objects: &Objects,
    items: &Items,
    icons: Option<&ItemDisplays>,
    rolls: crate::items::RollCatalogs,
    names: &crate::names::NameCache,
    sub_classes: Option<&crate::ui_items::ItemSubClasses>,
    classes: Option<&benilla_formats::ChrClasses>,
) -> InventorySlots {
    let has_relic_slot = store
        .0
        .unit_class()
        .is_some_and(|c| classes.is_some_and(|t| t.has_relic_slot(u32::from(c))));
    let mut inv: InventorySlots = Default::default();
    let ammo_id = store.0.player_ammo_id().unwrap_or(0);
    if ammo_id != 0 {
        // Only read: what asks the record are the texture and quality getters' lookups with the
        // callback `0x4c84f0` (`0x4c83ce`, `0x4c8a0a`), drained in `feed_char`.
        let t = items.template(ammo_id, 0, Ask::Read);
        let inventory_type = t.map(|t| t.inventory_type);
        let (name, quality, display, link) = match t {
            Some(t) => (
                Some(t.name.clone()),
                t.quality as i32,
                t.display_info_id,
                Some(item_link(ammo_id, &t.name, t.quality)),
            ),
            None => (None, 0, 0, None),
        };
        let icon = icons
            .and_then(|i| i.catalog.get(display))
            .and_then(|d| d.icon.clone());
        inv[0] = Some(InvSlotView {
            inventory_type,
            durability: None,
            flags: 0,
            // Ammo has no item object, only its template, so there is no bind to read.
            already_bound: false,
            bonding: 0,
            occupied_seats: None,
            use_spell: 0,
            use_fork: Default::default(),
            item_id: ammo_id,
            icon,
            count: ammo_count(store, objects, ammo_id),
            // `GetInventoryItemCount`'s `PLAYER_AMMO_ID` leg, before the container fork.
            contents_count: None,
            quality,
            name,
            link,
            // No click or drag path reaches the ammo slot: nothing locks it, it fits nowhere.
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
            spell_0: false,
            spell_charges: [0; 5],
        });
    }
    // Equipment 1..=19, then `Bag0Slot`..`Bag3Slot` (20..=23 in `PaperDollItemFrame.dbc`), the
    // equipped bag items: the live id is the inv slot plus one across the band.
    for live in 1..=23u32 {
        let Some(guid) = store.0.player_inv_slot(live as u8 - 1).filter(|g| *g != 0) else {
            continue;
        };
        inv[live as usize] = slot_view(
            objects,
            items,
            icons,
            rolls,
            names,
            sub_classes,
            has_relic_slot,
            guid,
        );
    }
    inv
}

/// The six bank-bag slots' views (live ids 64..=69). They are inventory slots, streamed whether
/// or not a banker is open, and the stock bank reads them through `BankButtonIDToInvSlotID(i, 1)`
/// (`BankFrame.lua:28-35, 199-204`), so they ride this feed rather than the bank window's state.
fn bank_bag_slots(
    store: &ObjectStore,
    objects: &Objects,
    items: &Items,
    icons: Option<&ItemDisplays>,
    rolls: crate::items::RollCatalogs,
    names: &crate::names::NameCache,
    sub_classes: Option<&crate::ui_items::ItemSubClasses>,
) -> BankBagSlots {
    let mut bags: BankBagSlots = Default::default();
    for (i, bag) in bags.iter_mut().enumerate() {
        let Some(guid) = store.0.player_bank_bag_slot(i as u8).filter(|g| *g != 0) else {
            continue;
        };
        *bag = slot_view(
            objects,
            items,
            icons,
            rolls,
            names,
            sub_classes,
            // A bank bag slot holds a bag; the relic flag decides INVSLOT 17 alone.
            false,
            guid,
        );
    }
    bags
}

pub(crate) fn feed_char(
    // `ChrClasses.dbc` field 16, the relic flag `find_equip_slot` reads; absent, no class has one.
    classes: Option<Res<crate::chr_classes::ChrClassTable>>,
    mut hosted_vm: crate::script_calls::HostedVm,
    mut inv: crate::items::Inventory,
    items: Res<Items>,
    icons: Option<Res<ItemDisplays>>,
    enchants: Option<Res<crate::items::Enchants>>,
    props: Option<Res<crate::items::RandomProperties>>,
    commands: Res<NetCommands>,
    mut feed: ResMut<CharFeedState>,
    names: Res<crate::names::NameCache>,
    // `ItemSubClass.dbc`, for `GetInventoryItemCount`'s container count bit.
    sub_classes: Option<Res<crate::ui_items::ItemSubClasses>>,
    // The ammo getters' record callback, fired with the field events after this push.
    mut field_notices: MessageWriter<crate::field_watch::FieldNotice>,
) {
    hosted_vm.run(|script| {
        let Some(script) = script else {
            return;
        };
        // The ammo slot getters' lookups with the callback `0x4c84f0` (`0x4c83bc`, `0x4c89cf`),
        // registered after the script's tick: a miss asks and waits for the answer; a record that
        // landed since the call runs the callback now, `UNIT_INVENTORY_CHANGED` (`0x4c8509`).
        for entry in script.take_ammo_record_asks() {
            if items.template(entry, 0, Ask::Event(&commands)).is_some() {
                field_notices.write(crate::field_watch::FieldNotice::Player {
                    id: crate::field_watch::id::UNIT_INVENTORY_CHANGED,
                });
            } else {
                items.wait_for_ammo_record(entry);
            }
        }
        // Per VM: after a `/reload` both snapshots re-push.
        let (memo, vm_reset) = feed.vm.get_reset(script);

        let Some(store) = inv.self_store.iter().next() else {
            // No self store is no data, not an empty inventory: it happens in the logout frames,
            // where `PLAYER_LOGOUT` handlers still read equipment, so nothing is cleared (the next
            // session starts a fresh VM).
            return;
        };

        // `GetWeaponEnchantInfo`'s data, pushed every frame outside the gate: the remaining time is
        // a live countdown, which the reference also recomputes per call (`0x5d9d00`). Pushed
        // first, so handlers the snapshots fire read the fresh value.
        let objects = &inv.objects;
        script.set_weapon_enchants(
            weapon_enchant(store, objects, EQUIPMENT_SLOT_MAINHAND),
            weapon_enchant(store, objects, EQUIPMENT_SLOT_OFFHAND),
        );

        // The gate, around the two snapshot builds only.
        let objects_moved = inv.changes.moved();
        let templates_moved = memo.items_templates.moved(items.template_epoch());
        let names_moved = memo.names_generation.moved(names.generation());
        // The display epoch, not a per-frame hold-open: the views read whole-second countdowns, so
        // they can change only when it steps.
        let deadlines_moved = memo.enchant_deadlines.moved(inv.changes.countdown_steps());
        let self_changed = !inv.self_changed.is_empty();
        // `is_added`, not `is_changed`: only the load-once icon column is read, and the model-cache
        // half churns every frame.
        let icons_added = icons.as_ref().is_some_and(|r| r.is_added());
        // Both catalogs load once at startup; one input covers the pair.
        let enchants_changed = enchants.as_ref().is_some_and(|r| r.is_changed())
            || props.as_ref().is_some_and(|r| r.is_changed());
        gate::trace(
            "feed_char",
            &[
                ("vm_reset", vm_reset),
                ("objects", objects_moved),
                ("templates", templates_moved),
                ("names", names_moved),
                ("deadlines", deadlines_moved),
                ("self", self_changed),
                ("icons", icons_added),
                ("enchants", enchants_changed),
            ],
        );
        let gate = gate::Gate::new(
            vm_reset
                || objects_moved
                || templates_moved
                || names_moved
                || deadlines_moved
                || self_changed
                || icons_added
                || enchants_changed,
        );
        if gate.skip() {
            return;
        }

        // The paper doll's getters read this; the stat events that repaint it are field watches,
        // fired after this push (`crate::ui_unit::field_events`).
        let stats = combat_stats(store, objects, &items);
        if memo.last_stats.as_ref() != Some(&stats) {
            gate.audit("feed_char", "the combat-stats snapshot");
            script.set_player_combat_stats(Some(stats.clone()));
            memo.last_stats = Some(stats);
        }

        let inv = inventory_slots(
            store,
            objects,
            &items,
            icons.as_deref(),
            crate::items::RollCatalogs {
                enchants: enchants.as_deref(),
                props: props.as_deref(),
            },
            &names,
            sub_classes.as_deref(),
            classes.as_deref().map(|t| &t.0),
        );
        let bank_bags = bank_bag_slots(
            store,
            objects,
            &items,
            icons.as_deref(),
            crate::items::RollCatalogs {
                enchants: enchants.as_deref(),
                props: props.as_deref(),
            },
            &names,
            sub_classes.as_deref(),
        );
        // The doll's `UNIT_INVENTORY_CHANGED` is its producers' alone, the visible-item bridge, the
        // ammunition watch and the item paths ([`crate::field_watch`], [`crate::item_watch`]),
        // fired after this push; a view change raises none, a repair's durability included, which
        // repaints through the push's `UPDATE_INVENTORY_ALERTS` (`0x5d9920` → `0x4c7ee0`).
        // `PLAYERBANKSLOTS_CHANGED`'s two producers, planned off the old memo: `false` is the
        // descriptor watcher (`0x5ddd6e`: a bag arrived, left or was swapped), `true` the
        // item-object path (`0x4c728d`: the same bag's own fields changed).
        let bank_bag_guids: [u64; BANK_BAG_SLOT_COUNT] =
            std::array::from_fn(|i| store.0.player_bank_bag_slot(i as u8).unwrap_or(0));
        let repainted: Vec<bool> = (0..BANK_BAG_SLOT_COUNT)
            .filter_map(|i| {
                if bank_bag_guids[i] != memo.last_bank_bag_guids[i] {
                    Some(false)
                } else {
                    let was = memo.last_bank_bags.as_ref().and_then(|b| b[i].as_ref());
                    (was != bank_bags[i].as_ref()).then_some(true)
                }
            })
            .collect();
        memo.last_bank_bag_guids = bank_bag_guids;
        if memo.last_inv.as_ref() != Some(&inv) || memo.last_bank_bags.as_ref() != Some(&bank_bags)
        {
            gate.audit("feed_char", "the inventory snapshot");
            script.set_inventory_slots(inv.clone());
            script.set_bank_bag_slots(bank_bags.clone());
            // The bank bag buttons repaint only on `PLAYERBANKSLOTS_CHANGED` and
            // `BANKFRAME_OPENED` (`BankFrame.lua:206-208`). One event per changed slot: the
            // descriptor watcher signals through `0x703e50` with no arguments, the item path
            // through `SignalEvent2` with `"player"`.
            memo.last_inv = Some(inv);
            memo.last_bank_bags = Some(bank_bags);
        }
        // Outside the push block: an identical-bag swap moves no view at all.
        if !repainted.is_empty() {
            gate.audit("feed_char", "a bank bag slot transition");
        }
        for same_bag in repainted {
            if same_bag {
                script.fire_event(
                    "PLAYERBANKSLOTS_CHANGED",
                    vec![ScriptValue::Str("player".to_string())],
                );
            } else {
                script.fire_event("PLAYERBANKSLOTS_CHANGED", vec![]);
            }
        }
    })
}

/// The 0-based weapon slots `GetWeaponEnchantInfo` (`0x4c9790`) reads (`0x4c97c3`, `0x4c988b`);
/// the frames' live ids are one higher.
const EQUIPMENT_SLOT_MAINHAND: u8 = 15;
const EQUIPMENT_SLOT_OFFHAND: u8 = 16;

/// The temporary enchantment slot (0 permanent, 2..6 random properties), as the reference's
/// remaining-time read passes it (`0x4c9828`).
const TEMP_ENCHANTMENT_SLOT: u8 = 1;

/// One weapon's temporary enchantment, read off the raw descriptor rather than
/// [`InvSlotView::enchants`], which drops unnamed ids and the totem imbues; the reference's only
/// gate is a nonzero id (`[descriptor+0x4c]`).
fn weapon_enchant(
    store: &ObjectStore,
    objects: &Objects,
    slot0: u8,
) -> Option<benilla_ui::script::WeaponEnchant> {
    let guid = store.0.player_inv_slot(slot0)?;
    // The deadline read, not the tooltip's: an elapsed timer answers 0, not "no timer".
    let remaining_ms = objects
        .countdowns(guid)
        .and_then(|c| c.enchant_deadline_ms(u32::from(TEMP_ENCHANTMENT_SLOT)));
    let obj = objects.object(guid)?;
    obj.item_enchant(TEMP_ENCHANTMENT_SLOT)?;
    Some(benilla_ui::script::WeaponEnchant {
        remaining_ms,
        charges: obj.item_enchant_charges(TEMP_ENCHANTMENT_SLOT),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut seats = |guid: u64| {
            slot_view(
                &objs.get(),
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
    use bevy::ecs::system::RunSystemOnce;

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
    /// `ITEM_FIELD_STACK_COUNT`, one of the six item fields the reference's watcher covers.
    const F_ITEM_STACK_COUNT: u16 = 14;
    /// Any container entry ("Traveler's Backpack"); the feed only needs it to resolve.
    const BAG_ENTRY: u32 = 4500;
    const BAG: u64 = 0x4000_0000_0000_0abc;

    /// Re-stream a live item object's fields as a values update does, moving its change tick.
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

    /// A world with `bag`, or nothing, in the first bank bag slot, and a frame recording events.
    fn world_with_bank_bag(bag: Option<u64>) -> App {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<CharFeedState>()
            .init_resource::<Items>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<crate::names::NameCache>()
            .add_message::<crate::field_watch::FieldNotice>()
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

    fn seen(app: &mut App) -> Vec<String> {
        let script = app.world_mut().non_send_resource_mut::<UiScript>();
        let out = script.eval::<Vec<String>>("return SEEN").unwrap();
        script.run("SEEN = {}").unwrap();
        out
    }

    #[test]
    fn a_bank_bag_slot_filling_fires_playerbankslots_changed() {
        let mut app = world_with_bank_bag(Some(BAG));
        app.world_mut().run_system_once(feed_char).unwrap();
        let events = seen(&mut app);
        assert!(
            events.contains(&"PLAYERBANKSLOTS_CHANGED nil".to_string()),
            "the repaint event, and it carries NO argument — `0x703e50` pushes none, and the \
             recorder stringifies the absent arg1 as nil. Got {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|e| e.starts_with("UNIT_INVENTORY_CHANGED")),
            "a bank bag seat is no doll seat (`0x4c726a`), got {events:?}"
        );
    }

    /// Rewrite the active player's descriptor wholesale: `ObjectFields` has no setter.
    fn set_self(app: &mut App, pairs: &[(u16, u32)]) {
        let mut q = app
            .world_mut()
            .query_filtered::<&mut ObjectStore, With<SelfPlayer>>();
        let mut store = q.single_mut(app.world_mut()).unwrap();
        *store = ObjectStore(ObjectFields::from_pairs(pairs));
    }

    /// A repair moves the doll's view and fires no `UNIT_INVENTORY_CHANGED`: the durability
    /// watcher `0x5d97c0` ends in the alert recompute (`0x5d9920`), whose
    /// `UPDATE_INVENTORY_ALERTS` repaints the slots. The doll's inventory event is its producers'
    /// alone ([`crate::field_watch`], [`crate::item_watch`]), never a change of this feed's view.
    #[test]
    fn a_repair_moves_the_doll_and_fires_no_inventory_event() {
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
        let broken = |app: &mut App| {
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .eval::<bool>(r#"return GetInventoryItemBroken("player", 1) ~= nil"#)
                .unwrap()
        };
        app.world_mut().run_system_once(feed_char).unwrap();
        assert!(broken(&mut app));
        let _ = seen(&mut app);
        restream(
            &mut app,
            HELM,
            ObjectFields::from_pairs(&[(F_OBJECT_ENTRY, HELM_ID), (DURABILITY, 40), (47, 40)]),
        );
        app.world_mut().run_system_once(feed_char).unwrap();
        assert!(!broken(&mut app), "the doll reads the repair");
        assert_eq!(seen(&mut app), Vec::<String>::new());
    }

    /// The ammo view only reads the record: what asks it are the texture and quality getters'
    /// lookups with the callback `0x4c84f0` (`0x4c83ce`, `0x4c8a0a`), so a frame no getter read
    /// the slot in sends nothing.
    #[test]
    fn the_ammo_view_reads_and_the_getters_ask() {
        const ARROW: u32 = 2512;
        /// `PLAYER_AMMO_ID`.
        const AMMO_ID: u16 = 1223;
        let mut app = world_with_bank_bag(None);
        let (tx, rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        set_self(&mut app, &[(AMMO_ID, ARROW)]);
        let asked = |rx: &crossbeam_channel::Receiver<crate::net::ClientCommand>| {
            crate::query_cache::queries(rx)
                .into_iter()
                .filter(|c| {
                    matches!(c, crate::net::ClientCommand::ItemQuery { entry, .. } if *entry == ARROW)
                })
                .count()
        };
        app.world_mut().run_system_once(feed_char).unwrap();
        app.world_mut().run_system_once(feed_char).unwrap();
        assert_eq!(asked(&rx), 0, "the view reads");
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run(r#"GetInventoryItemQuality("player", 0)"#)
            .unwrap();
        app.world_mut().run_system_once(feed_char).unwrap();
        assert_eq!(asked(&rx), 1, "the getter asks");
    }

    /// The ammo slot getters' record lookups with the callback `0x4c84f0` (`0x4c83bc`,
    /// `0x4c89cf`): a call that missed waits for the answer; one whose record landed since runs
    /// its callback at once, `UNIT_INVENTORY_CHANGED` with `"player"` (`0x4c8509`).
    #[test]
    fn an_ammo_getters_miss_waits_and_a_record_landed_since_signals_at_once() {
        use crate::field_watch::{id, FieldNotice};
        const ARROW: u32 = 2512;
        /// `PLAYER_AMMO_ID`.
        const AMMO_ID: u16 = 1223;
        let mut app = world_with_bank_bag(None);
        set_self(&mut app, &[(AMMO_ID, ARROW)]);
        app.world_mut().run_system_once(feed_char).unwrap();
        let ask = |app: &mut App| {
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .run(r#"GetInventoryItemQuality("player", 0)"#)
                .unwrap();
        };
        let signals = |app: &mut App| -> Vec<FieldNotice> {
            app.world_mut()
                .resource_mut::<Messages<FieldNotice>>()
                .drain()
                .collect()
        };
        ask(&mut app);
        app.world_mut().run_system_once(feed_char).unwrap();
        assert!(signals(&mut app).is_empty());
        assert_eq!(
            app.world_mut()
                .resource_mut::<Items>()
                .take_ammo_waiters(ARROW),
            1,
            "the miss waits for its answer"
        );
        ask(&mut app);
        app.world_mut()
            .resource_mut::<Items>()
            .insert_template(ARROW, Some(crate::items::test_template("Rough Arrow")));
        app.world_mut().run_system_once(feed_char).unwrap();
        assert_eq!(
            signals(&mut app),
            [FieldNotice::Player {
                id: id::UNIT_INVENTORY_CHANGED
            }]
        );
        assert_eq!(
            app.world_mut()
                .resource_mut::<Items>()
                .take_ammo_waiters(ARROW),
            0
        );
    }

    /// A bag's own field moving is the item-object path (`0x4c7180`), which pushes `"player"`.
    #[test]
    fn a_bank_bags_own_field_moving_fires_the_item_object_producer() {
        let mut app = world_with_bank_bag(Some(BAG));
        app.world_mut().run_system_once(feed_char).unwrap();
        assert!(seen(&mut app).contains(&"PLAYERBANKSLOTS_CHANGED nil".to_string()));

        // The same bag guid, restacked: only the item object moved.
        restream(
            &mut app,
            BAG,
            ObjectFields::from_pairs(&[(F_OBJECT_ENTRY, BAG_ENTRY), (F_ITEM_STACK_COUNT, 3)]),
        );
        app.world_mut().run_system_once(feed_char).unwrap();
        let events = seen(&mut app);
        assert!(
            events.contains(&"PLAYERBANKSLOTS_CHANGED player".to_string()),
            "the item path pushes the token, got {events:?}"
        );
    }

    /// Identical bags push identical views; the reference watches the guids and fires twice.
    #[test]
    fn two_identical_bags_swapped_between_bank_bag_slots_still_announce() {
        let mut app = world_with_bank_bag(Some(BAG));
        // A second, byte-identical bag in bank bag slot 2.
        const BAG2: u64 = 0x4000_0000_0000_0abd;
        // Rewritten wholesale: `ObjectFields` has no setter.
        let set_slots = |app: &mut App, a: u64, b: u64| {
            let mut q = app
                .world_mut()
                .query_filtered::<&mut ObjectStore, With<SelfPlayer>>();
            let mut store = q.single_mut(app.world_mut()).unwrap();
            *store = ObjectStore(ObjectFields::from_pairs(&[
                (F_BANK_BAG_1, a as u32),
                (F_BANK_BAG_1 + 1, (a >> 32) as u32),
                (F_BANK_BAG_1 + 2, b as u32),
                (F_BANK_BAG_1 + 3, (b >> 32) as u32),
            ]));
        };
        set_slots(&mut app, BAG, BAG2);
        crate::items::test_spawn_item(
            app.world_mut(),
            BAG2,
            ObjectFields::from_pairs(&[(F_OBJECT_ENTRY, BAG_ENTRY)]),
            true,
        );
        app.world_mut().run_system_once(feed_char).unwrap();
        let _ = seen(&mut app);

        // Exchange them. Every pushed view is equal before and after.
        set_slots(&mut app, BAG2, BAG);
        app.world_mut().run_system_once(feed_char).unwrap();
        let events = seen(&mut app);
        assert_eq!(
            events
                .iter()
                .filter(|e| *e == "PLAYERBANKSLOTS_CHANGED nil")
                .count(),
            2,
            "one argless event per slot whose guid moved, got {events:?}"
        );
    }

    /// The lock is the VM's, keyed by the guid this feed pushes: a lock moves no snapshot, and
    /// `IsInventoryItemLocked` reads it on the bank bag's own guid.
    #[test]
    fn a_bank_bag_reads_the_lock_on_its_own_guid() {
        let mut app = world_with_bank_bag(Some(BAG));
        app.world_mut().run_system_once(feed_char).unwrap();
        let _ = seen(&mut app);
        let locked = |app: &mut App| {
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .eval::<bool>(r#"return IsInventoryItemLocked(64) and true or false"#)
                .unwrap()
        };
        assert!(!locked(&mut app));
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .lock_item(BAG);
        app.world_mut().run_system_once(feed_char).unwrap();
        assert!(seen(&mut app).is_empty(), "a lock is no inventory change");
        assert!(locked(&mut app), "a send's lock on the bag");
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .unlock_item(BAG);
        assert!(!locked(&mut app), "the server's answer");
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
            app.world_mut().run_system_once(feed_char).unwrap();
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .eval::<Option<i64>>(r#"return GetInventoryItemQuality("player", 1)"#)
                .unwrap()
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
