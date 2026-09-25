//! The character-window feed: the [`UnitCombatStats`] and `InventorySlots` snapshots the paper
//! doll's Lua bindings read, built from our avatar's descriptor and item stores; the events that
//! repaint it; and the booth's yaw from the stock `CharacterModelFrame`. [`unit_combat_stats`] is
//! the half any unit shares: a creature has no player fields, so those read absent and keep their
//! defaults.

use bevy::prelude::*;

use benilla_protocol::messages::PLAYER_SKILL_SLOTS;
use benilla_ui::script::{
    weapon_subclass_skill, BankBagSlots, InvSlotView, InventorySlots, ScriptValue, SkillEntry,
    SkillsState, UiScript, UnitCombatStats, BANK_BAG_SLOT_COUNT, EQUIPMENT_BAG, SKILL_DEFENSE,
    SKILL_UNARMED,
};
use benilla_ui::strings::Arg;

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::{ClientCommand, NetCommands, ObjectStore, Objects, SelfPlayer};
use crate::pending_item_ops::PendingItemOps;
use crate::portrait::PaperDollBooth;
use crate::ui_items::{find_equip_slot, item_link};
use crate::ui_script::{gate, UiFeed, UiInput};

/// Equipment slots, 0-based (`EQUIPMENT_SLOT_*`): main hand, off hand, ranged.
const SLOT_MAIN_HAND: u8 = 15;
const SLOT_OFF_HAND: u8 = 16;
const SLOT_RANGED: u8 = 17;
/// Item class 2, weapon (vmangos `ItemPrototype.h:139`).
const ITEM_CLASS_WEAPON: u32 = 2;
/// Weapon subclass 19, wand (`ItemPrototype.h:211`), for `HasWandEquipped`.
const SUBCLASS_WAND: u32 = 19;

/// An f32 snapshot field as a diff key, quantized to 1/32 so equal wire values compare equal.
fn fixed(v: f32) -> i64 {
    (v * 32.0).round() as i64
}

/// The last snapshots pushed, so events fire on transitions only; held per VM
/// ([`crate::ui_script::VmMemo`]), so a `/reload` re-pushes and re-fires as a login does.
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
    /// `PendingItemOps::epoch`, beside `!is_empty()`: that holds the gate open while an op is in
    /// flight, this catches the frame a lock clears with no object moving
    /// (`SMSG_INVENTORY_CHANGE_FAILURE`).
    pending_epoch: gate::Watch,
}

/// The character-window feed; its bindings are `benilla-ui`'s `char_stats`.
pub(crate) struct UiCharPlugin;

impl Plugin for UiCharPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CharFeedState>().add_systems(
            Update,
            (
                // Before `feed_containers`: a `BAG_UPDATE` handler may read the bag item's own
                // row (`GetInventoryItemLink("player", ContainerIDToInventoryID(bag))`), so the
                // inventory push lands first.
                feed_char
                    .before(crate::ui_items::feed::feed_containers)
                    .in_set(UiFeed),
                watch_skill_ups.in_set(UiFeed),
                feed_skills.in_set(UiFeed),
                drain_skill_abandons.after(UiInput),
            ),
        );
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
fn skill_line(script: &UiScript, chat: &mut crate::ui_chat::ChatLog, key: &str, args: &[Arg<'_>]) {
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
    script: Option<NonSendMut<UiScript>>,
    self_guid: Res<crate::net::SelfGuid>,
    self_store: Query<&ObjectStore, With<SelfPlayer>>,
    skill_lines: Option<Res<crate::ui_spellbook::SkillLines>>,
    mut chat: ResMut<crate::ui_chat::ChatLog>,
    mut prev: Local<crate::ui_script::VmMemo<SkillBlock>>,
) {
    let (Some(guid), Ok(store)) = (self_guid.0, self_store.single()) else {
        return;
    };
    // The memo is per VM and a new VM re-seeds silently; with no VM nothing is diffed, or a stale
    // skill-up would replay at the next login. `SKILL_LINES_CHANGED` fires here only when a line
    // is added, removed or changes value; the reference fires it at the end of every skill-list
    // rebuild (`0x4d2fec`), which its value, max, permanent-bonus and temporary-bonus watchers
    // run (registered `0x5dda54`-`0x5ddaf0`), and from both header collapse and expand verbs.
    let Some(mut script) = script else {
        return;
    };
    let prev = prev.get(&script);
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
                                    &script,
                                    &mut chat,
                                    "ERR_SKILL_UP_SI",
                                    &[Arg::S(&name), Arg::D(i64::from(value))],
                                );
                            } else {
                                debug!("chat: skill-up silenced ({name} {old}→{value}, the 0x402 gate)");
                            }
                        }
                    }
                    // A new line: `ERR_SKILL_GAINED_S` (catalog row 53).
                    None => {
                        if let Some(name) = name() {
                            if announces() {
                                debug!("chat: skill-gain announced ({name} at {value})");
                                skill_line(
                                    &script,
                                    &mut chat,
                                    "ERR_SKILL_GAINED_S",
                                    &[Arg::S(&name)],
                                );
                            } else {
                                debug!(
                                    "chat: skill-gain silenced ({name} at {value}, the 0x402 gate)"
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
}

/// The skills-pane feed: every `PLAYER_SKILL_INFO` line the reference lists, as a flat
/// [`SkillEntry`] pushed through `set_skills` on change; the engine groups, sorts and folds.
fn feed_skills(
    script: Option<NonSendMut<UiScript>>,
    self_store: Query<Ref<ObjectStore>, With<SelfPlayer>>,
    skill_lines: Option<Res<crate::ui_spellbook::SkillLines>>,
    mut last: Local<crate::ui_script::VmMemo<Option<SkillsState>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let last = last.get(&script);
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
    })
}

/// Each queued `AbandonSkill` becomes one `CMSG_UNLEARN_SKILL`; nothing changes locally, as the
/// server's `SetSkill(id, 0, 0)` (vmangos `SkillHandler.cpp:69`) returns as a skill-field update.
fn drain_skill_abandons(script: Option<NonSendMut<UiScript>>, commands: Res<NetCommands>) {
    let Some(mut script) = script else {
        return;
    };
    for skill_id in script.take_skill_abandons() {
        debug!("ui_char: abandon skill line {skill_id}");
        let _ = commands.0.send(ClientCommand::UnlearnSkill { skill_id });
    }
}

/// The item entry in an equipped slot; `None` for an empty slot or an item not yet streamed.
fn slot_entry(store: &ObjectStore, objects: &Objects, slot0: u8) -> Option<u32> {
    let guid = store.0.player_inv_slot(slot0)?;
    objects.object(guid)?.object_entry()
}

/// The weapon-skill line for `slot0`: vmangos's subclass table (`Item.cpp:700`) for a weapon,
/// Unarmed (162) for anything else.
fn weapon_skill_id(
    store: &ObjectStore,
    objects: &Objects,
    items: &Items,
    commands: &NetCommands,
    slot0: u8,
) -> u32 {
    let Some(entry) = slot_entry(store, objects, slot0) else {
        return SKILL_UNARMED;
    };
    let Some(t) = items.template(entry, 0, commands) else {
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

/// A swing time in ms, with 0 or absent read as not yet streamed: the vanilla 2000, never zero,
/// as the reference divides damage by it.
fn base_swing(streamed: Option<u32>) -> u32 {
    streamed.filter(|&ms| ms != 0).unwrap_or(2000)
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
        // `0` counts as unstreamed: a created store reads an absent in-block field as `Some(0)`,
        // so a plain `unwrap_or` would leave `damage / speed` infinite.
        main_attack_time_ms: base_swing(f.unit_base_attack_time(0)),
        offhand_attack_time_ms: base_swing(f.unit_base_attack_time(1)),
        attack_power: f.unit_attack_power().unwrap_or(0),
        attack_power_pos: i32::from(ap_pos),
        attack_power_neg: i32::from(ap_neg),
        ranged_attack_power: f.unit_ranged_attack_power().unwrap_or(0),
        ranged_attack_power_pos: i32::from(rap_pos),
        ranged_attack_power_neg: i32::from(rap_neg),
        ranged_attack_time_ms: base_swing(f.unit_ranged_attack_time()),
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
fn combat_stats(
    store: &ObjectStore,
    objects: &Objects,
    items: &Items,
    commands: &NetCommands,
) -> UnitCombatStats {
    // The off hand counts only as a weapon (a shield does not swing); both flags read false
    // until the template lands.
    let has_offhand = slot_entry(store, objects, SLOT_OFF_HAND)
        .and_then(|e| items.template(e, 0, commands))
        .is_some_and(|t| t.class == ITEM_CLASS_WEAPON);
    let has_wand = slot_entry(store, objects, SLOT_RANGED)
        .and_then(|e| items.template(e, 0, commands))
        .is_some_and(|t| t.class == ITEM_CLASS_WEAPON && t.subclass == SUBCLASS_WAND);

    let main_skill = weapon_skill_id(store, objects, items, commands, SLOT_MAIN_HAND);
    let offhand_skill = weapon_skill_id(store, objects, items, commands, SLOT_OFF_HAND);
    let ranged_skill_id = slot_entry(store, objects, SLOT_RANGED)
        .and_then(|e| items.template(e, 0, commands))
        .filter(|t| t.class == ITEM_CLASS_WEAPON)
        .and_then(|t| weapon_subclass_skill(t.subclass));

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
    commands: &NetCommands,
    pending: &PendingItemOps,
    names: &crate::names::NameCache,
    // `ItemSubClass.dbc`, for the count gate below; absent, every bag counts 0.
    sub_classes: Option<&crate::ui_items::ItemSubClasses>,
    // The player's `ChrClasses.dbc` relic flag, which decides `find_equip_slot`'s INVSLOT 17.
    has_relic_slot: bool,
    guid: u64,
    live_id: u32,
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
    let creator = obj
        .item_creator()
        .filter(|&g| g != 0)
        .and_then(|g| names.resolve(g, commands).map(str::to_string));
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
    let t = items.template(entry, guid, commands);
    let (name, quality, display, link, equip_slots, kind, bar_placeable) = match t {
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
            )
        }
        None => (None, 0, 0, None, Vec::new(), (0, 0), false),
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
        locked: pending.contains(EQUIPMENT_BAG, live_id),
        equip_slots,
        bar_placeable,
        creator,
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
    commands: &NetCommands,
    pending: &PendingItemOps,
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
        let t = items.template(ammo_id, 0, commands);
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
            durability: None,
            flags: 0,
            // Ammo has no item object, only its template, so there is no bind to read.
            already_bound: false,
            item_id: ammo_id,
            icon,
            count: ammo_count(store, objects, ammo_id),
            // `GetInventoryItemCount`'s `PLAYER_AMMO_ID` leg, before the container fork.
            contents_count: None,
            quality,
            name,
            link,
            // No click or drag path reaches the ammo slot: nothing locks it, it fits nowhere.
            locked: false,
            equip_slots: Vec::new(),
            bar_placeable: false,
            creator: None,
            // No ammo instance streams, only its template id: no enchants, no countdown.
            enchants: Vec::new(),
            duration_ms: None,
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
            commands,
            pending,
            names,
            sub_classes,
            has_relic_slot,
            guid,
            live,
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
    commands: &NetCommands,
    pending: &PendingItemOps,
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
            commands,
            pending,
            names,
            sub_classes,
            // A bank bag slot holds a bag; the relic flag decides INVSLOT 17 alone.
            false,
            guid,
            BANK_BAG_LIVE_FIRST + i as u32,
        );
    }
    bags
}

/// `BankButtonIDToInvSlotID(1, isBag)`: the first of the six bank-bag slots' live ids.
const BANK_BAG_LIVE_FIRST: u32 = 64;

/// Fire the paper doll's repaint events for one unit's snapshot transition, `arg1 = token`: the
/// eight unit-stat events both pages register (`PaperDollFrame.lua:14-28`,
/// `PetPaperDollFrame.lua:12-20`), each when a value its handlers read moved. A first snapshot
/// fires them all.
pub(crate) fn fire_stat_transitions(
    script: &mut UiScript,
    token: &str,
    prev: Option<&UnitCombatStats>,
    stats: &UnitCombatStats,
) {
    let tok = || ScriptValue::Str(token.to_string());
    let changed = |sel: fn(&UnitCombatStats) -> Vec<i64>| prev.is_none_or(|p| sel(p) != sel(stats));
    if changed(|s| {
        let mut v: Vec<i64> = s.stats.iter().map(|&x| i64::from(x)).collect();
        v.extend(
            s.stat_pos
                .iter()
                .chain(s.stat_neg.iter())
                .map(|&x| i64::from(x)),
        );
        v
    }) {
        script.fire_event("UNIT_STATS", vec![tok()]);
    }
    if changed(|s| {
        s.resistances
            .iter()
            .chain(s.resistance_pos.iter())
            .chain(s.resistance_neg.iter())
            .map(|&x| i64::from(x))
            .collect()
    }) {
        script.fire_event("UNIT_RESISTANCES", vec![tok()]);
    }
    if changed(|s| {
        vec![
            fixed(s.min_damage),
            fixed(s.max_damage),
            fixed(s.min_offhand_damage),
            fixed(s.max_offhand_damage),
            i64::from(s.physical_bonus_pos),
            i64::from(s.physical_bonus_neg),
            fixed(s.damage_percent),
        ]
    }) {
        script.fire_event("UNIT_DAMAGE", vec![tok()]);
    }
    if changed(|s| {
        vec![
            i64::from(s.main_attack_time_ms),
            i64::from(s.offhand_attack_time_ms),
            i64::from(s.has_offhand),
        ]
    }) {
        script.fire_event("UNIT_ATTACK_SPEED", vec![tok()]);
    }
    if changed(|s| {
        vec![
            i64::from(s.attack_power),
            i64::from(s.attack_power_pos),
            i64::from(s.attack_power_neg),
        ]
    }) {
        script.fire_event("UNIT_ATTACK_POWER", vec![tok()]);
    }
    if changed(|s| {
        vec![
            i64::from(s.ranged_attack_power),
            i64::from(s.ranged_attack_power_pos),
            i64::from(s.ranged_attack_power_neg),
            i64::from(s.has_wand),
        ]
    }) {
        script.fire_event("UNIT_RANGED_ATTACK_POWER", vec![tok()]);
    }
    if changed(|s| {
        vec![
            fixed(s.ranged_min_damage),
            fixed(s.ranged_max_damage),
            i64::from(s.ranged_attack_time_ms),
        ]
    }) {
        script.fire_event("UNIT_RANGEDDAMAGE", vec![tok()]);
    }
    if changed(|s| {
        vec![
            i64::from(s.main_weapon_skill.0),
            i64::from(s.main_weapon_skill.1),
            i64::from(s.ranged_weapon_skill.0),
            i64::from(s.ranged_weapon_skill.1),
        ]
    }) {
        script.fire_event("UNIT_ATTACK", vec![tok()]);
    }
}

pub(crate) fn feed_char(
    // `ChrClasses.dbc` field 16, the relic flag `find_equip_slot` reads; absent, no class has one.
    classes: Option<Res<crate::chr_classes::ChrClassTable>>,
    script: Option<NonSendMut<UiScript>>,
    mut inv: crate::items::Inventory,
    items: Res<Items>,
    icons: Option<Res<ItemDisplays>>,
    enchants: Option<Res<crate::items::Enchants>>,
    props: Option<Res<crate::items::RandomProperties>>,
    commands: Res<NetCommands>,
    mut feed: ResMut<CharFeedState>,
    mut booth: ResMut<PaperDollBooth>,
    pending: Res<PendingItemOps>,
    names: Res<crate::names::NameCache>,
    // `ItemSubClass.dbc`, for `GetInventoryItemCount`'s container count bit.
    sub_classes: Option<Res<crate::ui_items::ItemSubClasses>>,
) {
    let Some(mut script) = script else {
        return;
    };
    // The stock `CharacterModelFrame` owns the yaw (`Model_Rotate*` call `SetRotation`); the booth
    // mirrors it, written only on change so it is not marked changed every frame.
    let yaw = script.model_pane_facing("CharacterModelFrame");
    if booth.yaw != yaw {
        booth.yaw = yaw;
    }

    // Per VM: after a `/reload` both snapshots re-push and their events re-fire.
    let (memo, vm_reset) = feed.vm.get_reset(&script);

    let Some(store) = inv.self_store.iter().next() else {
        // No self store is no data, not an empty inventory: it happens in the logout frames, where
        // `PLAYER_LOGOUT` handlers still read equipment, so nothing is cleared (the next session
        // starts a fresh VM).
        return;
    };

    // `GetWeaponEnchantInfo`'s data, pushed every frame outside the gate: the remaining time is a
    // live countdown, which the reference also recomputes per call (`0x5d9d00`). Pushed first, so
    // handlers the snapshots fire read the fresh value.
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
    let pending_held = !pending.is_empty();
    let pending_moved = memo.pending_epoch.moved(pending.epoch());
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
            ("pending", pending_held),
            ("pending-epoch", pending_moved),
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
            || enchants_changed
            || pending_held
            || pending_moved,
    );
    if gate.skip() {
        return;
    }

    let stats = combat_stats(store, objects, &items, &commands);
    if memo.last_stats.as_ref() != Some(&stats) {
        gate.audit("feed_char", "the combat-stats snapshot");
        // Push before firing: handlers run synchronously and would paint the old values.
        let prev = memo.last_stats.take();
        script.set_player_combat_stats(Some(stats.clone()));
        fire_stat_transitions(&mut script, "player", prev.as_ref(), &stats);
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
        &commands,
        &pending,
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
        &commands,
        &pending,
        &names,
        sub_classes.as_deref(),
    );
    // Both bands push under one `UNIT_INVENTORY_CHANGED`: one wire update, one event.
    // `PLAYERBANKSLOTS_CHANGED`'s two producers, planned off the old memo: `false` is the
    // descriptor watcher (`0x5ddd6e`: a bag arrived, left or was swapped), `true` the item-object
    // path (`0x4c728d`: the same bag's own fields changed).
    let bank_bag_guids: [u64; BANK_BAG_SLOT_COUNT] =
        std::array::from_fn(|i| store.0.player_bank_bag_slot(i as u8).unwrap_or(0));
    let repainted: Vec<bool> = (0..BANK_BAG_SLOT_COUNT)
        .filter_map(|i| {
            if bank_bag_guids[i] != memo.last_bank_bag_guids[i] {
                Some(false)
            } else {
                let was = memo.last_bank_bags.as_ref().and_then(|b| b[i].as_ref());
                (!same_item(was, bank_bags[i].as_ref())).then_some(true)
            }
        })
        .collect();
    memo.last_bank_bag_guids = bank_bag_guids;
    if memo.last_inv.as_ref() != Some(&inv) || memo.last_bank_bags.as_ref() != Some(&bank_bags) {
        gate.audit("feed_char", "the inventory snapshot");
        script.set_inventory_slots(inv.clone());
        script.set_bank_bag_slots(bank_bags.clone());
        script.fire_event(
            "UNIT_INVENTORY_CHANGED",
            vec![ScriptValue::Str("player".to_string())],
        );
        // The bank bag buttons repaint only on this and `BANKFRAME_OPENED`
        // (`BankFrame.lua:206-208`). One event per changed slot: the descriptor watcher signals
        // through `0x703e50` with no arguments, the item path through `SignalEvent2` with
        // `"player"`.
        memo.last_inv = Some(inv);
        memo.last_bank_bags = Some(bank_bags);
    }
    // Outside the push block: an identical-bag swap moves no view at all.
    if !repainted.is_empty() {
        gate.audit("feed_char", "a bank bag slot transition");
    }
    for same_bag in repainted {
        let args = if same_bag {
            vec![ScriptValue::Str("player".to_string())]
        } else {
            Vec::new()
        };
        script.fire_event("PLAYERBANKSLOTS_CHANGED", args);
    }
}

/// Whether two bank-bag views hold the same item; `locked` is `ITEM_LOCK_CHANGED`'s business.
fn same_item(a: Option<&InvSlotView>, b: Option<&InvSlotView>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            let unlocked = |v: &InvSlotView| InvSlotView {
                locked: false,
                ..v.clone()
            };
            unlocked(a) == unlocked(b)
        }
        _ => false,
    }
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
            .init_resource::<PaperDollBooth>()
            .init_resource::<PendingItemOps>()
            .init_resource::<Items>()
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
            events.contains(&"UNIT_INVENTORY_CHANGED player".to_string()),
            "and the doll's, unchanged, got {events:?}"
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

    /// A lock flip is `ITEM_LOCK_CHANGED`'s, not a slot change.
    #[test]
    fn a_lock_flip_alone_does_not_fire_the_repaint_event() {
        let mut app = world_with_bank_bag(Some(BAG));
        app.world_mut().run_system_once(feed_char).unwrap();
        let _ = seen(&mut app);

        // The same bag, now locked by an outbound move.
        app.world_mut().resource_mut::<PendingItemOps>().add([(
            EQUIPMENT_BAG,
            BANK_BAG_LIVE_FIRST,
            BAG,
            1,
        )]);
        app.world_mut().run_system_once(feed_char).unwrap();
        let events = seen(&mut app);
        assert!(
            events.contains(&"UNIT_INVENTORY_CHANGED player".to_string()),
            "the snapshot did move — `locked` is part of it, got {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|e| e.starts_with("PLAYERBANKSLOTS_CHANGED")),
            "…but the ITEM did not change, so the repaint event stays quiet, got {events:?}"
        );
    }

    /// The frame a lock clears, `!pending.is_empty()` already reads false;
    /// [`PendingItemOps::epoch`] is what still runs the feed.
    #[test]
    fn the_frame_a_lock_clears_still_repushes_the_slot() {
        let mut app = world_with_bank_bag(Some(BAG));
        app.world_mut().resource_mut::<PendingItemOps>().add([(
            EQUIPMENT_BAG,
            BANK_BAG_LIVE_FIRST,
            BAG,
            1,
        )]);
        app.world_mut().run_system_once(feed_char).unwrap();
        let _ = seen(&mut app);
        assert!(
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .eval::<bool>(r#"return IsInventoryItemLocked(64) and true or false"#)
                .unwrap(),
            "in flight, the slot reads locked"
        );

        // A refusal: the lock clears without any object moving.
        let cleared = app
            .world_mut()
            .resource_mut::<PendingItemOps>()
            .clear_by_failure(BAG);
        assert_eq!(cleared, vec![(EQUIPMENT_BAG, BANK_BAG_LIVE_FIRST)]);
        app.world_mut().run_system_once(feed_char).unwrap();
        assert!(
            !app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .eval::<bool>(r#"return IsInventoryItemLocked(64) and true or false"#)
                .unwrap(),
            "the unlock has to reach the VM in the same frame it happened"
        );
    }
}
