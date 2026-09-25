//! The per-frame feeds into the VM: the container snapshots built from the player's descriptor,
//! and the item-tooltip views every window reads.

use std::collections::HashMap;

use bevy::prelude::*;

use benilla_protocol::ItemInfo;
use benilla_ui::script::{ContainerSlot, ContainerState, ScriptValue, UiScript};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::{NetCommands, ObjectStore, Objects, SelfPlayer};
use crate::pending_item_ops::{LockTransitions, PendingItemOps};
use crate::ui_script::gate;

use super::equip_error::equip_error_key;
use super::{
    find_equip_slot, has_key, keyring_size, slot_guid_count, EquipErrors, BAGS, BAG_SLOT_FIRST,
    BANK_BAGS, BANK_BAG_ID_FIRST, BANK_BAG_SLOT_FIRST, BANK_CONTAINER, BANK_SLOTS,
    KEYRING_CONTAINER, KEYRING_SLOTS, PACK_SLOTS,
};

/// What the container feed last pushed, for per-bag change events.
#[derive(Default)]
pub(crate) struct FeedMemory {
    pushed: HashMap<i64, ContainerState>,
    /// The last `HasKey()` pushed, so its flip is logged once.
    had_key: bool,
    /// Counters for the stores whose lazy resolves make `is_changed` always true.
    items_templates: gate::Watch,
    cooldown_epoch: gate::Watch,
    names_generation: gate::Watch,
    petition_records: gate::Watch,
    enchant_deadlines: gate::Watch,
    /// The last [`SlotGuids::bags`], diffed for `BAG_CLOSED`.
    bag_guids: [u64; 10],
    /// The last [`SlotGuids::vault`], diffed for `PLAYERBANKSLOTS_CHANGED`.
    vault_guids: [u64; BANK_SLOTS as usize],
}

/// The item guids in the player's bag and vault slots, as the reference's watchers read them
/// (`0x5dd8a0` installs `0x5ddcf0` over the guid fields): which item, where a pushed slot view only
/// says what it looks like, so two identical items swapped still register.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(crate) struct SlotGuids {
    /// The bag in containers 1..=10 (equipped, then bank bags), indexed `id - 1`.
    pub bags: [u64; 10],
    /// The 24 vault slots, indexed `slot - 1`.
    pub vault: [u64; BANK_SLOTS as usize],
}

/// A spell's `$`-substituted description, for item trigger and set-bonus lines. An empty one
/// yields `None` and the caller drops the whole line, prefix included: the item builder
/// (`0x52d8a0`) expands it (`0x5075f0`), tests the first byte (`0x52da29`) and on zero skips the
/// trigger block (`0x52da31`), so an undescribed spell such as a key's `Opening` prints no line.
fn spell_desc_text(
    spells: Option<&crate::ui_action::Spells>,
    id: u32,
    home_area: Option<&str>,
    // The VM's strings for the keyed `$d`/`$s` tokens.
    text: &dyn Fn(&str, &[i64]) -> Option<String>,
) -> Option<String> {
    let sp = spells?;
    let d = sp.catalog.get(id)?;
    match d.description.as_deref().filter(|t| !t.is_empty()) {
        // The builder tests the expanded text, which can be empty from a non-empty template.
        Some(desc) => {
            let ctx = benilla_formats::TokenContext {
                durations: &sp.durations,
                radii: &sp.radii,
                lookup: &|i| sp.catalog.get(i),
                home_area,
                text,
            };
            Some(benilla_formats::substitute(desc, d, &ctx)).filter(|t| !t.is_empty())
        }
        None => None,
    }
}

/// The tooltip's "N Charges" count, 0 for no line. The builder (`0x52d8a0`) reads a template 0 as
/// `-1` (`0x52da01`), prints nothing for `-1` (`0x52db51`) and otherwise the absolute value
/// (`0x52db56`): food's `-1` shows no line, a `-5` pool "5 Charges". It prints per spell slot; one
/// line here, as no vmangos `item_template` row has two pools.
fn charges_count(spells: &[benilla_protocol::messages::ItemSpellEntry]) -> i32 {
    spells
        .iter()
        .find(|s| s.spell_id != 0 && s.charges != 0 && s.charges != -1)
        .map(|s| s.charges.abs())
        .unwrap_or(0)
}

/// A reputation rank (0..=7) to `FACTION_STANDING_LABEL{rank+1}`, the unsuffixed key the reference
/// builds (`0x84b5dc`), from the player's own `GlobalStrings.lua`.
fn standing_label(rank: u32, get: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    get(&format!("FACTION_STANDING_LABEL{}", rank.min(7) + 1))
}

/// An item template's tooltip view: the wire fields plus the strings only the app resolves, the
/// trigger-spell text and the skill, spell and reputation requirements.
fn template_view(
    t: &ItemInfo,
    spells: Option<&crate::ui_action::Spells>,
    skill_lines: Option<&benilla_formats::SkillLineCatalog>,
    home_area: Option<&str>,
    factions: Option<&benilla_formats::FactionCatalog>,
    sub_classes: Option<&benilla_formats::ItemSubClassCatalog>,
    classes: Option<&benilla_formats::ItemClassCatalog>,
    icons: Option<&ItemDisplays>,
    // The VM's own `GlobalStrings.lua`, for the reputation-requirement line.
    get: &dyn Fn(&str) -> Option<String>,
    // The same table, for the `$`-engine's keyed tokens.
    text: &dyn Fn(&str, &[i64]) -> Option<String>,
) -> benilla_ui::script::ItemTemplateView {
    let spell_name = |id: u32| -> Option<String> {
        spells
            .and_then(|s| s.catalog.get(id))
            .map(|sd| sd.name.clone())
    };
    let spell_text = |id: u32| spell_desc_text(spells, id, home_area, text);
    benilla_ui::script::ItemTemplateView {
        name: t.name.clone(),
        quality: t.quality,
        class: t.class,
        subclass: t.subclass,
        inventory_type: t.inventory_type,
        proficiency_alt: sub_classes.and_then(|c| c.proficiency_alt(t.class, t.subclass)),
        hide_subclass: sub_classes.is_some_and(|c| c.hides_name(t.class, t.subclass)),
        // `GetItemInfo`'s type pair: the subclass is `name()`, verbose name first (`0x48e311`),
        // so a one-handed sword is "One-Handed Swords" to an addon and "Sword" on the tooltip.
        item_type: classes.and_then(|c| c.name(t.class)).map(str::to_string),
        item_sub_type: sub_classes
            .and_then(|c| c.name(t.class, t.subclass))
            .map(str::to_string),
        // The tooltip's spelling: the display name alone.
        sub_class_display: sub_classes
            .and_then(|c| c.display_name(t.class, t.subclass))
            .map(str::to_string),
        flags: t.flags,
        bonding: t.bonding,
        max_count: t.max_count,
        // The stack size, `GetItemInfo`'s `itemStackCount`; `max_count` is the unique cap.
        stackable: t.stackable,
        // `GetItemInfo`'s `itemTexture`, the bag slots' `ItemDisplayInfo.dbc` icon.
        icon: icons
            .and_then(|i| i.catalog.get(t.display_info_id))
            .and_then(|d| d.icon.clone()),
        start_quest: t.start_quest,
        container_slots: t.container_slots,
        stats: t.stats.clone(),
        damages: t.damages.iter().map(|d| (d.min, d.max, d.school)).collect(),
        delay_ms: t.delay_ms,
        armor: t.armor,
        block: t.block,
        resistances: t.resistances,
        max_durability: t.max_durability,
        required_level: t.required_level,
        allowable_class: t.allowable_class,
        allowable_race: t.allowable_race,
        required_skill: t.required_skill,
        required_skill_rank: t.required_skill_rank,
        required_skill_name: (t.required_skill != 0)
            .then(|| {
                skill_lines
                    .and_then(|sl| sl.line(t.required_skill))
                    .map(|l| l.name.clone())
            })
            .flatten(),
        required_spell: t.required_spell,
        required_spell_name: (t.required_spell != 0)
            .then(|| spell_name(t.required_spell))
            .flatten(),
        required_honor_rank: t.required_honor_rank,
        required_city_rank: t.required_city_rank,
        // `ITEM_REQ_REPUTATION`, the reference's key for this line (`0x854b8c`): faction, then
        // standing; an install missing a key shows no line.
        required_rep_line: (t.required_rep_faction != 0)
            .then(|| {
                let faction = factions.and_then(|c| c.faction_name(t.required_rep_faction))?;
                let standing = standing_label(t.required_rep_rank, get)?;
                Some(benilla_ui::strings::fill(
                    &get("ITEM_REQ_REPUTATION")?,
                    &[
                        benilla_ui::strings::Arg::S(faction),
                        benilla_ui::strings::Arg::S(&standing),
                    ],
                ))
            })
            .flatten(),
        required_rep_faction: t.required_rep_faction,
        required_rep_rank: t.required_rep_rank,
        lock_id: t.lock_id,
        spell_triggers: t
            .spells
            .iter()
            .filter(|s| s.spell_id != 0)
            .filter_map(|s| spell_text(s.spell_id).map(|n| (s.trigger, s.spell_id, n)))
            .collect(),
        charges: charges_count(&t.spells),
        description: t.description.clone(),
        page_text: t.page_text,
        sell_price: t.sell_price,
        item_set: t.item_set,
        random_property: t.random_property,
    }
}

/// Answers the engine's set-id asks from `ItemSet.dbc`: member names from the template cache, each
/// miss queried as the reference queries set members, and bonus text from the `$`-engine. A set
/// stays pending, re-pushing, until every member resolves.
pub(super) fn feed_item_sets(
    script: Option<NonSendMut<UiScript>>,
    sets: Option<Res<super::ItemSets>>,
    items: Res<Items>,
    commands: Res<NetCommands>,
    spells: Option<Res<crate::ui_action::Spells>>,
    skill_lines: Option<Res<crate::ui_spellbook::SkillLines>>,
    mut pending: Local<
        crate::ui_script::VmMemo<std::collections::HashMap<u32, benilla_ui::script::ItemSetView>>,
    >,
) {
    let Some(mut script) = script else {
        return;
    };
    let pending = pending.get(&script);
    for id in script.take_item_set_asks() {
        pending.entry(id).or_default();
    }
    if pending.is_empty() {
        return;
    }
    let Some(sets) = sets.as_deref() else {
        return; // no catalog without client data: the asks stay parked
    };
    let spell_res = spells.as_deref();
    let skill_catalog = skill_lines.as_deref().map(|s| &s.catalog);
    let mut done: Vec<u32> = Vec::new();
    let mut push: Vec<(u32, benilla_ui::script::ItemSetView)> = Vec::new();
    // Dropped before the push, which needs the VM mutably.
    let token_text = crate::ui_script::token_text(&script);
    for (&set_id, last) in pending.iter_mut() {
        let Some(row) = sets.0.set(set_id) else {
            done.push(set_id); // no such row: drop the ask
            continue;
        };
        let members: Vec<(u32, Option<String>)> = row
            .items
            .iter()
            .map(|&id| (id, items.template(id, 0, &commands).map(|t| t.name.clone())))
            .collect();
        let view = benilla_ui::script::ItemSetView {
            name: row.name.clone(),
            bonuses: row
                .bonuses
                .iter()
                .filter_map(|&(n, spell)| {
                    spell_desc_text(spell_res, spell, None, &token_text).map(|desc| (n, desc))
                })
                .collect(),
            required_skill: row.required_skill,
            required_skill_rank: row.required_skill_rank,
            required_skill_name: (row.required_skill != 0)
                .then(|| {
                    skill_catalog
                        .and_then(|sl| sl.line(row.required_skill))
                        .map(|l| l.name.clone())
                })
                .flatten(),
            members,
        };
        if view.members.iter().all(|(_, n)| n.is_some()) {
            done.push(set_id);
        }
        if *last != view {
            *last = view.clone();
            push.push((set_id, view));
        }
    }
    drop(token_text);
    for (id, view) in push {
        script.set_item_set(id, view);
    }
    for id in done {
        pending.remove(&id);
    }
}

/// Pushes the whole random-suffix table once per VM, when both catalogs are in: its consumers are
/// click-driven, and a chat-link tooltip has no hover loop to repaint a late answer.
pub(super) fn feed_random_properties(
    script: Option<NonSendMut<UiScript>>,
    props: Option<Res<crate::items::RandomProperties>>,
    enchants: Option<Res<crate::items::Enchants>>,
    mut pushed: Local<crate::ui_script::VmMemo<bool>>,
) {
    let Some(mut script) = script else {
        return;
    };
    if *pushed.get(&script) {
        return;
    }
    // Without the enchant catalog every row would push empty, never to be retried.
    let (Some(props), Some(enchants)) = (props, enchants) else {
        return;
    };
    let rows = crate::items::random_property_views(&props, Some(&enchants));
    let with_lines = rows.values().filter(|v| !v.enchants.is_empty()).count();
    info!(
        "random-property table: {} rows pushed, {with_lines} with enchant lines",
        rows.len()
    );
    script.set_random_properties(rows);
    *pushed.get(&script) = true;
}

/// The item-tooltip feed: every template that lands is pushed unprompted, so the first hover never
/// misses, as the reference reads one item cache synchronously; a read of an unresolved id records
/// a miss, which queries it here.
pub(super) fn feed_item_stats(
    script: Option<NonSendMut<UiScript>>,
    mut items: ResMut<Items>,
    commands: Res<NetCommands>,
    spells: Option<Res<crate::ui_action::Spells>>,
    skill_lines: Option<Res<crate::ui_spellbook::SkillLines>>,
    // The `$z` token: the bind point's area (`SMSG_BINDPOINTUPDATE`), named through `AreaTable`.
    home_bind: Option<Res<crate::net::HomeBind>>,
    area_names: Option<Res<crate::ui_quest_log::QuestHeaderNamesRes>>,
    factions: Option<Res<crate::target::Factions>>,
    sub_classes: Option<Res<super::ItemSubClasses>>,
    // `GetItemInfo`'s `itemType` and `itemTexture`.
    classes: Option<Res<super::ItemClasses>>,
    icons: Option<Res<ItemDisplays>>,
    mut pending: Local<crate::ui_script::VmMemo<std::collections::HashSet<u32>>>,
    mut last_home: Local<crate::ui_script::VmMemo<Option<String>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let pending = pending.get(&script);
    let last_home = last_home.get(&script);
    // `GetBindLocation()`'s push, here so it and the `$z` token share one name, and ahead of the
    // pending gate below: the bind point can arrive while the feed idles.
    let home_area: Option<&str> = home_bind
        .as_deref()
        .and_then(|b| b.0)
        .and_then(|id| area_names.as_deref()?.0.resolve(id as i32));
    // Pushed on change, per VM: a rebuilt VM is fed again the frame it appears.
    if last_home.as_deref() != home_area {
        script.set_bind_location(home_area.unwrap_or_default());
        *last_home = home_area.map(str::to_string);
        // Re-substitute every held view, or one pushed before the bind point carries a raw `$z`.
        pending.extend(items.cached_template_ids());
    }

    pending.extend(items.take_fresh());
    pending.extend(script.take_item_stat_asks());
    if pending.is_empty() {
        return;
    }
    let spell_res = spells.as_deref();
    let skill_catalog = skill_lines.as_deref().map(|s| &s.catalog);
    let ready: Vec<u32> = pending
        .iter()
        .copied()
        .filter(|&id| items.template(id, 0, &commands).is_some())
        .collect();
    // Built first, pushed after: the lookup borrows the VM that `set_item_template` needs mutably.
    let views: Vec<(u32, benilla_ui::script::ItemTemplateView)> = {
        let get = |key: &str| {
            script
                .lua()
                .globals()
                .get::<String>(key)
                .ok()
                .filter(|t| !t.is_empty())
        };
        ready
            .into_iter()
            .filter_map(|id| {
                pending.remove(&id);
                let t = items.template(id, 0, &commands)?.clone();
                Some((
                    id,
                    template_view(
                        &t,
                        spell_res,
                        skill_catalog,
                        home_area,
                        factions.as_deref().map(|f| f.catalog()),
                        sub_classes.as_deref().map(|s| &s.0),
                        classes.as_deref().map(|c| &c.0),
                        icons.as_deref(),
                        &get,
                        &crate::ui_script::token_text(&script),
                    ),
                ))
            })
            .collect()
    };
    for (id, view) in views {
        script.set_item_template(id, view);
    }
}

/// The player state the tooltip's red requirement lines check: level, class, race, skills,
/// proficiencies (`SMSG_SET_PROFICIENCY`) and reputation ranks (the DBC base plus the
/// `SMSG_INITIALIZE_FACTIONS` standing), pushed on change.
pub(super) fn feed_player_req(
    script: Option<NonSendMut<UiScript>>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    changed_self: Query<(), (With<SelfPlayer>, Changed<ObjectStore>)>,
    proficiencies: Res<crate::net::Proficiencies>,
    reputations: Res<crate::net::Reputations>,
    factions: Option<Res<crate::target::Factions>>,
    actions: Res<crate::ui_action::PlayerActions>,
    spells: Option<Res<crate::ui_action::Spells>>,
    mut last: Local<crate::ui_script::VmMemo<Option<benilla_ui::script::PlayerReqState>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let (last, vm_reset) = last.get_reset(&script);
    // The state is a function of these inputs alone.
    let self_changed = !changed_self.is_empty();
    let prof_changed = proficiencies.is_changed();
    let reps_changed = reputations.is_changed();
    let factions_changed = factions.as_ref().is_some_and(|r| r.is_changed());
    let actions_changed = actions.is_changed();
    let spells_changed = spells.as_ref().is_some_and(|r| r.is_changed());
    gate::trace(
        "feed_player_req",
        &[
            ("vm_reset", vm_reset),
            ("self", self_changed),
            ("proficiencies", prof_changed),
            ("reputations", reps_changed),
            ("factions", factions_changed),
            ("actions", actions_changed),
            ("spells", spells_changed),
        ],
    );
    let gate = gate::Gate::new(
        vm_reset
            || self_changed
            || prof_changed
            || reps_changed
            || factions_changed
            || actions_changed
            || spells_changed,
    );
    if gate.skip() {
        return;
    }
    let Some(store) = self_q.iter().next() else {
        return;
    };
    let mut skills = std::collections::HashMap::new();
    for slot in 0..benilla_protocol::messages::PLAYER_SKILL_SLOTS {
        let Some(s) = store.0.player_skill(slot) else {
            continue;
        };
        if s.skill_id != 0 {
            // The requirement checks (`0x5eaae0`, `0x5ea930`) read the value plus the permanent
            // bonus, never the temporary one; the bonus is never negative in live data.
            skills.insert(
                u32::from(s.skill_id),
                u32::from(s.value).saturating_add_signed(i32::from(s.perm_bonus)),
            );
        }
    }
    let race = store.0.unit_race().unwrap_or(0);
    let class = store.0.unit_class().unwrap_or(0);
    let mut rep_ranks = std::collections::HashMap::new();
    if let Some(cat) = factions.as_deref().map(|f| f.catalog()) {
        for (id, info) in cat.reputation_factions() {
            let standing = usize::try_from(info.rep_index)
                .ok()
                .and_then(|i| reputations.0.get(i))
                .map(|&(_, s)| s)
                .unwrap_or(0);
            rep_ranks.insert(
                id,
                benilla_formats::reputation_rank(info.base_for(race, class) + standing),
            );
        }
    }
    // The reference's `0xc4d770`: a known spell whose Effect[0] is 40 (`SPELL_EFFECT_DUAL_WIELD`).
    let can_dual_wield = spells.as_deref().is_some_and(|s| {
        actions
            .spells
            .iter()
            .any(|&id| s.catalog.get(id).is_some_and(|sd| sd.effects[0] == 40))
    });
    let state = benilla_ui::script::PlayerReqState {
        level: store.0.unit_level().unwrap_or(0),
        class_id: u32::from(class),
        race_id: u32::from(race),
        skills,
        proficiency: proficiencies.0.clone(),
        rep_ranks,
        can_dual_wield,
        honor_rank: store.0.player_honor_rank().unwrap_or(0),
    };
    if last.as_ref() != Some(&state) {
        gate.audit("feed_player_req", "the player-requirement state");
        *last = Some(state.clone());
        script.set_player_req_state(state);
    }
}

/// One bag slot from its item guid: instance, template, icon and use-spell cooldown. `None` is an
/// empty slot; an occupied one not yet resolved is `Some` with empty fields.
fn resolve_slot(
    guid: u64,
    objects: &Objects,
    items: &Items,
    icons: Option<&ItemDisplays>,
    rolls: crate::items::RollCatalogs,
    commands: &NetCommands,
    cooldowns: &crate::spell::Cooldowns,
    spells: Option<&benilla_formats::SpellCatalog>,
    names: &crate::names::NameCache,
    // Mutable: reading a charter's record is what queries it.
    petitions: &mut crate::ui_petition::PetitionState,
    now: std::time::Instant,
    ui_now: f64,
    // The class's `ChrClasses.dbc` relic flag, which decides slot 17 for every fit-rule read.
    has_relic_slot: bool,
) -> Option<ContainerSlot> {
    if guid == 0 {
        return None;
    }
    // The enchant and lifetime (`SMSG_ITEM_TIME_UPDATE`) countdowns, in whole seconds.
    let countdowns = objects.countdowns(guid);
    let enchant_ms: [Option<u64>; crate::items::ENCHANT_SLOTS] =
        std::array::from_fn(|s| countdowns.and_then(|c| c.enchant_remaining_display_ms(s as u32)));
    let duration_ms = countdowns.and_then(|c| c.lifetime_remaining_display_ms());
    let (entry, count, durability, readable, creator, flags, already_bound, roll, enchant_lines) =
        match objects.object(guid) {
            Some(fields) => (
                fields.object_entry().unwrap_or(0),
                fields.item_stack_count().unwrap_or(1),
                // Max 0 is indestructible: no line.
                fields
                    .item_durability()
                    .zip(fields.item_max_durability())
                    .filter(|&(_, max)| max > 0),
                // Letter text on the instance, which the hover magnifier keys off; a template
                // `PageText` book does not set it.
                fields.item_text_id().is_some_and(|id| id != 0),
                // The creator's name for "<Made by %s>" and "Written by %s", `None` until the
                // name query answers.
                fields
                    .item_creator()
                    .filter(|&g| g != 0)
                    .and_then(|g| names.resolve(g, commands).map(str::to_string)),
                // The tooltip's unlocked (0x4) and wrapped (0x8) bits.
                fields.item_flags().unwrap_or(0),
                // `0x5da2c0`: soulbound, or carrying a binding enchant, off the raw descriptor.
                crate::items::already_bound(fields, rolls.enchants),
                // The roll behind the name's suffix; its enchants are already in slots 2..6.
                fields.item_random_properties_id(),
                // All seven enchant slots with charges and countdowns: our own items stream whole,
                // where others' carry two slots.
                crate::items::enchant_lines(
                    (0..7).map(|s| {
                        (
                            s,
                            fields.item_enchant(s).unwrap_or(0),
                            fields.item_enchant_charges(s),
                            enchant_ms[usize::from(s)],
                        )
                    }),
                    rolls.enchants,
                ),
            ),
            // A guid whose create has not landed: occupied, unresolved.
            None => return Some(ContainerSlot::default()),
        };
    if entry == 0 {
        return Some(ContainerSlot::default());
    }
    let template: Option<ItemInfo> = items.template(entry, guid, commands).cloned();
    let Some(t) = template else {
        // Asked (or a cached negative); show the slot occupied while the answer is in flight.
        return Some(ContainerSlot {
            durability: None,
            item_id: entry,
            count,
            readable,
            creator,
            flags,
            already_bound,
            enchants: enchant_lines,
            duration_ms,
            ..Default::default()
        });
    };
    Some(ContainerSlot {
        // A charter's guild name and master, keyed by the petition id the server puts in
        // enchantment slot 0 (`PetitionsHandler.cpp:126`), where the reference reads it
        // (`0x5ef337`) and its enchant lines skip it (`0x52c9e0`). The first hover sends the query.
        petition: (t.flags & benilla_protocol::messages::ITEM_FLAG_CHARTER != 0)
            .then(|| {
                let id = objects
                    .object(guid)
                    .and_then(|f| f.item_enchant(0))
                    .unwrap_or(0);
                petitions.tooltip_view(id as u32, guid, names, commands)
            })
            .flatten(),
        texture: icons
            .and_then(|i| i.catalog.get(t.display_info_id))
            .and_then(|d| d.icon.clone()),
        count,
        durability,
        quality: Some(t.quality),
        item_id: entry,
        // The link carries the roll in `randomPropertyId` and in the suffixed name (`0x5d8b00`);
        // the tooltip reads its name back off it.
        link: Some(crate::ui_items::item_link_full(
            entry,
            0,
            roll,
            0,
            &rolls.name(&t.name, roll),
            t.quality,
        )),
        locked: false,
        readable,
        creator,
        flags,
        already_bound,
        enchants: enchant_lines,
        duration_ms,
        equip_slots: find_equip_slot(t.inventory_type, has_relic_slot),
        bar_placeable: t.placeable_on_action_bar(),
        cooldown: t.use_spell.and_then(|u| {
            let sd = spells.and_then(|s| s.get(u.spell_id));
            cooldowns
                .info(u.spell_id, entry, sd, now)
                .ui_triple(now, ui_now)
        }),
    })
}

/// Reason 16's `%s`, the target bag's `BagFamily` name ("Only Arrows can be placed in that."), or
/// `None` to keep the generic line. `bag_slot` is the bag's player-array slot, and the reference's
/// `0x5ede00` names nothing for 255 (the player's own array) or for a slot at or past
/// `[player+0x1d38]`, 113 for the local player, so equipped bags (19..22) and bank bags (63..68)
/// both resolve. It reads the bag family, not the subclass the errorId's name suggests.
fn bag_family_name(
    player: Option<&ObjectStore>,
    bag_slot: u8,
    objects: &Objects,
    items: &Items,
    families: Option<&benilla_formats::ItemBagFamilyCatalog>,
    commands: &NetCommands,
) -> Option<String> {
    let store = player?;
    let families = families?;
    let guid = match bag_slot {
        s if (BAG_SLOT_FIRST..BAG_SLOT_FIRST + BAGS).contains(&s) => store.0.player_inv_slot(s),
        s if (BANK_BAG_SLOT_FIRST..BANK_BAG_SLOT_FIRST + BANK_BAGS).contains(&s) => {
            store.0.player_bank_bag_slot(s - BANK_BAG_SLOT_FIRST)
        }
        // 255, the player's own array, names no bag.
        _ => None,
    }
    .filter(|&g| g != 0)?;
    let entry = objects.object(guid)?.object_entry().filter(|&e| e != 0)?;
    let family = items.template(entry, guid, commands)?.bag_family;
    families.name(family).map(str::to_string)
}

/// The pending locks' resolving clear: releases every op whose slots have moved on and queues the
/// unlocks for [`feed_containers`] to announce. Ordered ahead of both feeds: `feed_char` pushes the
/// doll's and bank bags' `locked` from the same set, and a clear after it strands a stale lock.
pub(crate) fn resolve_item_locks(
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    objects: Objects,
    mut pending: ResMut<PendingItemOps>,
    mut transitions: ResMut<LockTransitions>,
) {
    if pending.is_empty() {
        return;
    }
    let player = self_q.iter().next();
    if player.is_none() {
        return;
    }
    transitions
        .0
        .extend(pending.resolve(|bag, slot1| slot_guid_count(player, bag, slot1, &objects)));
}

#[allow(clippy::type_complexity)] // the param list is the input set
pub(crate) fn feed_containers(
    script: Option<NonSendMut<UiScript>>,
    // `ChrClasses.dbc` field 16, the fit rule's relic flag.
    classes: Option<Res<crate::chr_classes::ChrClassTable>>,
    items: Res<Items>,
    icons: Option<Res<ItemDisplays>>,
    // The tuples below pair parameters under Bevy's 16-parameter ceiling.
    catalogs: (
        Option<Res<crate::items::Enchants>>,
        Option<Res<crate::items::RandomProperties>>,
    ),
    mut inv: crate::items::Inventory,
    commands: Res<NetCommands>,
    cooldowns: Res<crate::spell::Cooldowns>,
    spells: Option<Res<crate::ui_action::Spells>>,
    equip: (ResMut<EquipErrors>, crate::ui_action::MessageSink),
    // Reason 16's `%s` source; absent, every 16 keeps the generic line.
    bag_families: Option<Res<crate::ui_items::ItemBagFamilies>>,
    pending: Res<PendingItemOps>,
    queues: (ResMut<LockTransitions>, ResMut<super::net::BagOpens>),
    names: Res<crate::names::NameCache>,
    // The petition cache is mutable: a charter's hover is what queries it.
    clock_and_petitions: (
        Res<crate::ui_script::UiClock>,
        ResMut<crate::ui_petition::PetitionState>,
    ),
    mut memory: Local<crate::ui_script::VmMemo<FeedMemory>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let (clock, mut petitions) = clock_and_petitions;
    let (mut equip_errors, mut sink) = equip;
    let (mut lock_cleared, mut opens) = queues;
    let (memory, vm_reset) = memory.get_reset(&script);
    // The snapshot is a function of these inputs; the stores' counters stand in for their
    // `is_changed`, which lazy resolves make always true. `UiClock` is not an input: a running
    // cooldown's pushed start is absolute, and expiry moves `feed_epoch` through the prune.
    let objects_moved = inv.changes.moved();
    let templates_moved = memory.items_templates.moved(items.template_epoch());
    let cooldowns_moved = memory.cooldown_epoch.moved(cooldowns.feed_epoch());
    let names_moved = memory.names_generation.moved(names.generation());
    let petitions_moved = memory.petition_records.moved(petitions.records_epoch());
    // Countdowns are pushed in whole seconds, so only a displayed change reopens the gate.
    let deadlines_moved = memory
        .enchant_deadlines
        .moved(inv.changes.countdown_steps());
    let sweep = cooldowns.sweep_pending(clock.anchor);
    let self_changed = !inv.self_changed.is_empty();
    // `is_added`, not `is_changed`: only the load-once icon catalog is read here, and the
    // resource's model cache changes every frame.
    let icons_changed = icons.as_ref().is_some_and(|r| r.is_added());
    // Both catalogs load once, at startup: one input covers the pair.
    let enchants_changed = catalogs.0.as_ref().is_some_and(|r| r.is_changed())
        || catalogs.1.as_ref().is_some_and(|r| r.is_changed());
    let spells_changed = spells.as_ref().is_some_and(|r| r.is_changed());
    let families_changed = bag_families.as_ref().is_some_and(|r| r.is_changed());
    let errors_held = !equip_errors.0.is_empty();
    let locks_held = !lock_cleared.0.is_empty() || !pending.is_empty() || !opens.0.is_empty();
    gate::trace(
        "feed_containers",
        &[
            ("vm_reset", vm_reset),
            ("objects", objects_moved),
            ("templates", templates_moved),
            ("cooldowns", cooldowns_moved),
            ("sweep", sweep),
            ("names", names_moved),
            ("petition-records", petitions_moved),
            ("deadlines", deadlines_moved),
            ("self", self_changed),
            ("icons", icons_changed),
            ("enchants", enchants_changed),
            ("spells", spells_changed),
            ("families", families_changed),
            ("errors", errors_held),
            ("locks", locks_held),
        ],
    );
    let gate = gate::Gate::new(
        vm_reset
            || objects_moved
            || templates_moved
            || cooldowns_moved
            // The frame a timer crosses zero, before the prune moves the epoch.
            || sweep
            || names_moved
            || petitions_moved
            || deadlines_moved
            || self_changed
            || icons_changed
            || enchants_changed
            || spells_changed
            || families_changed
            || errors_held
            || locks_held,
    );
    if gate.skip() {
        return;
    }
    // `WOW_FEED_COST=1` prints the body's mean cost every 60 runs.
    let cost_t0 = std::env::var_os("WOW_FEED_COST")
        .is_some()
        .then(std::time::Instant::now);
    // One clock pair for every slot, so a running cooldown's pushed start is frame-stable.
    let (now, ui_now) = (clock.anchor, clock.ui_now);
    let spell_catalog = spells.as_deref().map(|s| &s.catalog);
    let rolls = crate::items::RollCatalogs {
        enchants: catalogs.0.as_deref(),
        props: catalogs.1.as_deref(),
    };
    // Refusals become the red error line through `equip_error_key`. An empty or missing string
    // prints nothing, as the reference's sink guard (`0x4945b4`) drops it; that silences 59.
    let player = inv.self_store.iter().next();
    let mut refusals = Vec::new();
    for e in equip_errors.0.drain(..) {
        // Reason 16 names the bag's family when the bag resolves, as `0x5ede00` does.
        let subclass_fill = (e.reason == 16)
            .then(|| {
                bag_family_name(
                    player,
                    e.bag_slot,
                    &inv.objects,
                    &items,
                    bag_families.as_deref().map(|c| &c.0),
                    &commands,
                )
            })
            .flatten();
        let key = match subclass_fill {
            Some(_) => "ERR_WRONG_BAG_TYPE_SUBCLASS",
            None => equip_error_key(e.reason),
        };
        let text = script
            .lua()
            .globals()
            .get::<String>(key)
            .unwrap_or_default();
        if text.is_empty() {
            continue;
        }
        // Reason 1 fills `%d` and 16 `%s`; neither carries the other's fill.
        let text = match e.required_level {
            Some(d) => text.replace("%d", &d.to_string()),
            None => text,
        };
        let text = match subclass_fill {
            Some(family) => text.replace("%s", &family),
            None => text,
        };
        refusals.push(crate::ui_action::Shown::keyed(key, text));
    }
    // Through the one sink, which also plays the error speech 18 of these keys carry.
    crate::ui_action::show_messages(&mut script, &mut sink, "ui_items", refusals);
    // The frame's unlocks, from `resolve_item_locks` and the inventory-failure handler, fire
    // `ITEM_LOCK_CHANGED` after the slot push below and after `feed_char`'s, so a repaint sees
    // the unlocked slot; server-opened bags fire `BAG_OPEN(id)` then too.
    let transitioned: Vec<(i64, u32)> = std::mem::take(&mut lock_cleared.0);
    let opened: Vec<i64> = std::mem::take(&mut opens.0);

    let mut fresh: HashMap<i64, ContainerState> = HashMap::new();
    // The class's relic flag; without client data it is false, an ordinary ranged slot.
    let has_relic_slot = player.is_some_and(|p| {
        p.0.unit_class().is_some_and(|c| {
            classes
                .as_deref()
                .is_some_and(|t| t.0.has_relic_slot(u32::from(c)))
        })
    });
    if let Some(store) = player {
        let mut slots = HashMap::new();
        for i in 0..PACK_SLOTS {
            let guid = store.0.player_pack_slot(i).unwrap_or(0);
            if let Some(mut slot) = resolve_slot(
                guid,
                &inv.objects,
                &items,
                icons.as_deref(),
                rolls,
                &commands,
                &cooldowns,
                spell_catalog,
                &names,
                &mut petitions,
                now,
                ui_now,
                has_relic_slot,
            ) {
                slot.locked = pending.contains(0, u32::from(i) + 1);
                slots.insert(u32::from(i) + 1, slot);
            }
        }
        fresh.insert(
            0,
            ContainerState {
                name: Some("Backpack".into()),
                num_slots: u32::from(PACK_SLOTS),
                slots,
            },
        );

        // Bags 1..4: each INV bag slot holds a container object with its own slot array.
        for bag in 1..=BAGS {
            let bag_guid = store
                .0
                .player_inv_slot(BAG_SLOT_FIRST + bag - 1)
                .unwrap_or(0);
            if bag_guid == 0 {
                continue; // no bag equipped → absent → GetContainerNumSlots = 0
            }
            let (entry, num_slots, slot_guids) = match inv.objects.object(bag_guid) {
                Some(f) => {
                    let n = f.container_num_slots().unwrap_or(0);
                    let guids: Vec<u64> = (0..n.min(36) as u8)
                        .map(|j| f.container_slot(j).unwrap_or(0))
                        .collect();
                    (f.object_entry().unwrap_or(0), n, guids)
                }
                None => (0, 0, Vec::new()),
            };
            let name = (entry != 0)
                .then(|| {
                    items
                        .template(entry, bag_guid, &commands)
                        .map(|t| t.name.clone())
                })
                .flatten();
            let mut slots = HashMap::new();
            for (j, &guid) in slot_guids.iter().enumerate() {
                if let Some(mut slot) = resolve_slot(
                    guid,
                    &inv.objects,
                    &items,
                    icons.as_deref(),
                    rolls,
                    &commands,
                    &cooldowns,
                    spell_catalog,
                    &names,
                    &mut petitions,
                    now,
                    ui_now,
                    has_relic_slot,
                ) {
                    slot.locked = pending.contains(i64::from(bag), j as u32 + 1);
                    slots.insert(j as u32 + 1, slot);
                }
            }
            fresh.insert(
                i64::from(bag),
                ContainerState {
                    name,
                    num_slots,
                    slots,
                },
            );
        }

        // The bank, container -1 (the vault) and 5..=10 (its bags), fed whether or not the window
        // is open: the descriptor streams at login.
        let mut slots = HashMap::new();
        for i in 0..BANK_SLOTS {
            let guid = store.0.player_bank_slot(i).unwrap_or(0);
            if let Some(mut slot) = resolve_slot(
                guid,
                &inv.objects,
                &items,
                icons.as_deref(),
                rolls,
                &commands,
                &cooldowns,
                spell_catalog,
                &names,
                &mut petitions,
                now,
                ui_now,
                has_relic_slot,
            ) {
                slot.locked = pending.contains(BANK_CONTAINER, u32::from(i) + 1);
                slots.insert(u32::from(i) + 1, slot);
            }
        }
        fresh.insert(
            BANK_CONTAINER,
            ContainerState {
                name: Some("Bank".into()),
                num_slots: u32::from(BANK_SLOTS),
                slots,
            },
        );
        for bank_bag in 0..BANK_BAGS {
            let bag_id = BANK_BAG_ID_FIRST + i64::from(bank_bag);
            let bag_guid = store.0.player_bank_bag_slot(bank_bag).unwrap_or(0);
            if bag_guid == 0 {
                continue; // no bag in the slot → absent → GetContainerNumSlots = 0
            }
            let (entry, num_slots, slot_guids) = match inv.objects.object(bag_guid) {
                Some(f) => {
                    let n = f.container_num_slots().unwrap_or(0);
                    let guids: Vec<u64> = (0..n.min(36) as u8)
                        .map(|j| f.container_slot(j).unwrap_or(0))
                        .collect();
                    (f.object_entry().unwrap_or(0), n, guids)
                }
                None => (0, 0, Vec::new()),
            };
            let name = (entry != 0)
                .then(|| {
                    items
                        .template(entry, bag_guid, &commands)
                        .map(|t| t.name.clone())
                })
                .flatten();
            let mut slots = HashMap::new();
            for (j, &guid) in slot_guids.iter().enumerate() {
                if let Some(mut slot) = resolve_slot(
                    guid,
                    &inv.objects,
                    &items,
                    icons.as_deref(),
                    rolls,
                    &commands,
                    &cooldowns,
                    spell_catalog,
                    &names,
                    &mut petitions,
                    now,
                    ui_now,
                    has_relic_slot,
                ) {
                    slot.locked = pending.contains(bag_id, j as u32 + 1);
                    slots.insert(j as u32 + 1, slot);
                }
            }
            fresh.insert(
                bag_id,
                ContainerState {
                    name,
                    num_slots,
                    slots,
                },
            );
        }

        // The keyring, container -2: player slots 81.., sized by the level ladder that stock
        // `GetKeyRingSize` (`ContainerFrame.lua:773`) reads off `UnitLevel` and the server shares.
        // Slots past the size, which the server refuses, are not fed.
        let size = keyring_size(store.0.unit_level().unwrap_or(1));
        let mut slots = HashMap::new();
        for i in 0..size.min(u32::from(KEYRING_SLOTS)) as u8 {
            let guid = store.0.player_keyring_slot(i).unwrap_or(0);
            if let Some(mut slot) = resolve_slot(
                guid,
                &inv.objects,
                &items,
                icons.as_deref(),
                rolls,
                &commands,
                &cooldowns,
                spell_catalog,
                &names,
                &mut petitions,
                now,
                ui_now,
                has_relic_slot,
            ) {
                slot.locked = pending.contains(KEYRING_CONTAINER, u32::from(i) + 1);
                slots.insert(u32::from(i) + 1, slot);
            }
        }
        fresh.insert(
            KEYRING_CONTAINER,
            ContainerState {
                name: Some("Keyring".into()),
                num_slots: size,
                slots,
            },
        );

        // `HasKey()`, which decides whether the keyring shows at all, pushed with the containers
        // so it is fresh on every `BAG_UPDATE` frame.
        let key = has_key(&store.0, &inv.objects, &items, &commands);
        if key != memory.had_key {
            gate.audit("feed_containers", "the HasKey() flip");
            debug!(
                "ui_items: HasKey() -> {key} (keyring {})",
                if key { "shown" } else { "hidden" }
            );
            memory.had_key = key;
        }
        script.set_has_key(key);
    }

    if let Some(t0) = cost_t0 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SUM_NS: AtomicU64 = AtomicU64::new(0);
        static N: AtomicU64 = AtomicU64::new(0);
        let sum = SUM_NS.fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);
        let n = N.fetch_add(1, Ordering::Relaxed) + 1;
        if n.is_multiple_of(60) {
            let slots: usize = fresh.values().map(|c| c.slots.len()).sum();
            eprintln!(
                "[feed-cost] ms={:.3} slots={slots} containers={}",
                sum as f64 / n as f64 / 1e6,
                fresh.len(),
            );
        }
    }
    if apply_container_source(
        &mut script,
        memory,
        player.is_some().then_some(fresh),
        SlotGuids {
            bags: std::array::from_fn(|i| player.map_or(0, |s| bag_slot_guid(s, i as i64 + 1))),
            vault: std::array::from_fn(|i| player.map_or(0, |s| vault_slot_guid(s, i as u8))),
        },
        transitioned,
        opened,
    ) {
        gate.audit(
            "feed_containers",
            "a bag diff, a lock event or a server-opened bag",
        );
    }
}

/// Diffs `source` against the last push, pushes each changed bag and fires the reference's events:
/// `BAG_UPDATE(bagID)` (`PLAYERBANKSLOTS_CHANGED` for the vault), the lock transitions, `BAG_OPEN`.
///
/// `None` is an absent self store (before login, or the logout despawn frames before the VM shuts
/// down), never "no items": the VM keeps its last push, as the reference's UI shutdown
/// (`0x490bd0`) runs with the inventory intact, where a burst of size-0 `BAG_UPDATE`s would have a
/// bag-saving addon record empty bags. Lock events still flush, never to fire into a later VM.
pub(crate) fn apply_container_source(
    script: &mut UiScript,
    memory: &mut FeedMemory,
    source: Option<HashMap<i64, ContainerState>>,
    // Beside the source, since an absent source must leave the guid memory alone too.
    guids: SlotGuids,
    transitioned: Vec<(i64, u32)>,
    opened: Vec<i64>,
) -> bool {
    // A lock transition also flips a slot's `locked`, so it shows in the diff, but its
    // `ITEM_LOCK_CHANGED` fires below regardless.
    let mut pushed = false;
    if let Some(fresh) = source {
        pushed |= diff_and_push(script, memory, fresh, guids);
    }
    // After the push, so a repaint reads the corrected `locked`.
    pushed |= !transitioned.is_empty();
    // No arguments: the reference's five fire sites (`0x495415`, `0x495455`, `0x49557d`,
    // `0x5d859a`, `0x5d94b9`) call `FrameScript_SignalEvent` (`0x703e50`), which pushes none, and
    // stock consumers repaint from `this` (`ContainerFrame.lua:39`, `PaperDollFrame.lua:601`,
    // `BankFrame.lua:209`).
    for _ in transitioned {
        script.fire_event("ITEM_LOCK_CHANGED", Vec::new());
    }
    // The reference fires `BAG_OPEN(id)` from the handler (`0x5e3b35`); here it follows this
    // frame's `BAG_UPDATE`s, so the frame shown paints the bag as it is.
    pushed |= !opened.is_empty();
    for bag in opened {
        script.fire_event("BAG_OPEN", vec![ScriptValue::Int(bag)]);
    }
    // Whether anything went into the VM, for the caller's gate audit.
    pushed
}

/// The item guid in vault slot `i` (0-based; player slots 39..62), 0 when empty. The reference's
/// band `0x5dd8a0` watches these fields with `0x5ddcf0`.
fn vault_slot_guid(store: &ObjectStore, i: u8) -> u64 {
    store.0.player_bank_slot(i).unwrap_or(0)
}

/// The guid of the bag in container `id` (1..=10, equipped then bank bags), 0 when empty or for any
/// other id. These are the ten slots whose watcher (`0x4f8cc0` installs `0x4f8ec0`) can close a
/// window; the backpack and keyring loops (`0x4f8db0`) never reach the event.
fn bag_slot_guid(store: &ObjectStore, id: i64) -> u64 {
    match id {
        1..=4 => store.0.player_inv_slot(BAG_SLOT_FIRST + (id as u8 - 1)),
        5..=10 => store.0.player_bank_bag_slot(id as u8 - 5),
        _ => None,
    }
    .unwrap_or(0)
}

/// The present-source half of [`apply_container_source`]: push every changed bag, announce each.
fn diff_and_push(
    script: &mut UiScript,
    memory: &mut FeedMemory,
    fresh: HashMap<i64, ContainerState>,
    guids: SlotGuids,
) -> bool {
    // `BAG_CLOSED` (one fire site, `0x4f92b5`) is the only thing that hides a bag window. It fires
    // when the bag's guid changes from nonzero (`0x4f923d`, `0x4f9247`), so a swapped bag closes
    // and then updates; diffed on the guid, as two identical bags swapped leave the states equal.
    let closed: Vec<i64> = (1..=10)
        .filter(|&id| {
            let (was, now) = (
                memory.bag_guids[id as usize - 1],
                guids.bags[id as usize - 1],
            );
            was != 0 && now != was
        })
        .collect();
    // A removed bag fires `BAG_CLOSED` and no `BAG_UPDATE` (`0x4f92cc`).
    let emptied: Vec<i64> = closed
        .iter()
        .copied()
        .filter(|&id| guids.bags[id as usize - 1] == 0)
        .collect();
    memory.bag_guids = guids.bags;
    for id in &closed {
        script.fire_event("BAG_CLOSED", vec![ScriptValue::Int(*id)]);
    }
    // `PLAYERBANKSLOTS_CHANGED` has two producers in the reference: the slot's guid changing
    // (`0x5ddcf0` fires `0x5ddd6e`) with no arguments, and the same item's fields changing
    // (`0x4c7180` fires `0x4c728d`) with `arg1 = "player"`. So the slot's guid decides, not its
    // view: two identical stacks swapped push equal views and still fire the argless one twice.
    // Planned before `memory` is rewritten, fired after the push, outside the `changed` gate.
    let vault: Vec<bool> = {
        let empty = HashMap::new();
        let now = fresh.get(&BANK_CONTAINER).map_or(&empty, |c| &c.slots);
        let was = memory
            .pushed
            .get(&BANK_CONTAINER)
            .map_or(&empty, |c| &c.slots);
        (1..=u32::from(BANK_SLOTS))
            .filter_map(|slot| {
                let i = slot as usize - 1;
                if guids.vault[i] != memory.vault_guids[i] {
                    Some(false)
                } else {
                    (now.get(&slot) != was.get(&slot)).then_some(true)
                }
            })
            .collect()
    };
    memory.vault_guids = guids.vault;
    let changed: Vec<i64> = fresh
        .keys()
        .chain(memory.pushed.keys())
        .copied()
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .filter(|b| fresh.get(b) != memory.pushed.get(b))
        .collect();
    if !changed.is_empty() {
        // A stack-count write on an item we own fires `ITEM_LOCK_CHANGED` ahead of `BAG_UPDATE`
        // (the reference's mirror handler `0x5d9400`, registered at `0x5d9360` via `0x468070`):
        // an auto-shot timer's only per-shot signal. Computed before `memory.pushed` is replaced.
        let restacked = {
            let empty = HashMap::new();
            let mut v: Vec<(i64, u32)> = Vec::new();
            for &bag in &changed {
                let now = fresh.get(&bag).map_or(&empty, |c| &c.slots);
                let was = memory.pushed.get(&bag).map_or(&empty, |c| &c.slots);
                for (&slot, n) in now {
                    // The same item restacked; a changed entry is a create or a swap.
                    if was.get(&slot).is_some_and(|w| {
                        w.item_id != 0 && w.item_id == n.item_id && w.count != n.count
                    }) {
                        v.push((bag, slot));
                    }
                }
            }
            // Sorted: an event stream has to be reproducible.
            v.sort_unstable();
            v
        };
        for &bag in &changed {
            script.set_container(bag, fresh.get(&bag).cloned());
        }
        // Ahead of `BAG_UPDATE`, one argless event per restacked slot.
        for _ in &restacked {
            script.fire_event("ITEM_LOCK_CHANGED", Vec::new());
        }
        debug!(
            "ui_items: fed {} changed bag(s) — {}",
            changed.len(),
            changed
                .iter()
                .map(|b| format!("{b}:{}", fresh.get(b).map_or(0, |c| c.slots.len())))
                .collect::<Vec<_>>()
                .join(" ")
        );
        for &bag in &changed {
            // Every changed bag but the vault fires `BAG_UPDATE`: `0x4f8cc0` installs no container
            // listener over the vault's fields, which fire only `PLAYERBANKSLOTS_CHANGED`.
            if bag != BANK_CONTAINER && !emptied.contains(&bag) {
                script.fire_event("BAG_UPDATE", vec![ScriptValue::Int(bag)]);
            }
        }
        memory.pushed = fresh;
    }
    // One event per changed vault slot, with no slot argument: every bank button repaints from
    // its own slot (`feed_char` fires the same event for the bank-bag buttons).
    for &same_item in &vault {
        let args = if same_item {
            vec![ScriptValue::Str("player".into())]
        } else {
            Vec::new()
        };
        script.fire_event("PLAYERBANKSLOTS_CHANGED", args);
    }
    // A `BAG_CLOSED` or vault event alone still went into the VM.
    !changed.is_empty() || !closed.is_empty() || !vault.is_empty()
}

#[cfg(test)]
mod tests {
    use super::{
        apply_container_source, charges_count, ContainerSlot, FeedMemory, SlotGuids,
        BANK_CONTAINER, BANK_SLOTS,
    };

    /// No bags and an empty vault.
    const NO_BAGS: SlotGuids = SlotGuids {
        bags: [0; 10],
        vault: [0; BANK_SLOTS as usize],
    };

    /// The handler's container ids reach the VM as `BAG_OPEN`'s `arg1`.
    #[test]
    fn a_server_opened_bag_fires_bag_open_with_its_container_id() {
        let mut s = UiScript::new().unwrap();
        s.run(
            "SEEN = {} \
             local f = CreateFrame('Frame') \
             f:RegisterEvent('BAG_OPEN') \
             f:RegisterEvent('BAG_UPDATE') \
             f:SetScript('OnEvent', function() \
                 table.insert(SEEN, event .. ' ' .. tostring(arg1)) end)",
        )
        .unwrap();
        let mut memory = FeedMemory::default();
        // A bag equipped into the second bag slot, and nothing else changed this frame.
        assert!(apply_container_source(
            &mut s,
            &mut memory,
            None,
            NO_BAGS,
            Vec::new(),
            vec![2]
        ));
        let seen = s.eval::<Vec<String>>("return SEEN").unwrap();
        assert_eq!(seen, vec!["BAG_OPEN 2"]);
        // Nothing queued: nothing fired, and the push reports nothing went in.
        s.run("SEEN = {}").unwrap();
        assert!(!apply_container_source(
            &mut s,
            &mut memory,
            None,
            NO_BAGS,
            Vec::new(),
            Vec::new()
        ));
        assert!(s.eval::<Vec<String>>("return SEEN").unwrap().is_empty());
    }

    /// A new guid fires the argless event, a restack fires it with `"player"`, and two identical
    /// items swapped fire the argless one twice though the views are equal.
    #[test]
    fn playerbankslots_changed_names_its_producer_by_the_slot_guid() {
        let mut s = UiScript::new().unwrap();
        s.run(
            "SEEN = {} \
             local f = CreateFrame('Frame') \
             f:RegisterEvent('PLAYERBANKSLOTS_CHANGED') \
             f:SetScript('OnEvent', function() \
                 table.insert(SEEN, event .. ' ' .. tostring(arg1)) end)",
        )
        .unwrap();
        let seen = |s: &mut UiScript| {
            let out = s.eval::<Vec<String>>("return SEEN").unwrap();
            s.run("SEEN = {}").unwrap();
            out
        };
        // The vault, with `slot` holding `count` of item 1234.
        let vault = |slot: u32, count: u32| {
            HashMap::from([(
                BANK_CONTAINER,
                ContainerState {
                    name: None,
                    num_slots: u32::from(BANK_SLOTS),
                    slots: HashMap::from([(
                        slot,
                        ContainerSlot {
                            item_id: 1234,
                            count,
                            ..Default::default()
                        },
                    )]),
                },
            )])
        };
        let mut guids = NO_BAGS;
        let mut memory = FeedMemory::default();

        // An item arrives in vault slot 1: the descriptor path, no arguments.
        guids.vault[0] = 0xF00D;
        apply_container_source(
            &mut s,
            &mut memory,
            Some(vault(1, 5)),
            guids,
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(seen(&mut s), vec!["PLAYERBANKSLOTS_CHANGED nil"]);

        // The same item restacks: the item watcher's path, with the unit token.
        apply_container_source(
            &mut s,
            &mut memory,
            Some(vault(1, 9)),
            guids,
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(seen(&mut s), vec!["PLAYERBANKSLOTS_CHANGED player"]);

        apply_container_source(
            &mut s,
            &mut memory,
            Some(vault(1, 9)),
            guids,
            Vec::new(),
            Vec::new(),
        );
        assert!(seen(&mut s).is_empty(), "an unchanged vault says nothing");

        // Two identical stacks swapped between slots 1 and 2: the guids are the only witness.
        let two = || {
            HashMap::from([(
                BANK_CONTAINER,
                ContainerState {
                    name: None,
                    num_slots: u32::from(BANK_SLOTS),
                    slots: HashMap::from([
                        (
                            1,
                            ContainerSlot {
                                item_id: 1234,
                                count: 9,
                                ..Default::default()
                            },
                        ),
                        (
                            2,
                            ContainerSlot {
                                item_id: 1234,
                                count: 9,
                                ..Default::default()
                            },
                        ),
                    ]),
                },
            )])
        };
        guids.vault[1] = 0xBEEF;
        apply_container_source(
            &mut s,
            &mut memory,
            Some(two()),
            guids,
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(seen(&mut s), vec!["PLAYERBANKSLOTS_CHANGED nil"]);
        guids.vault.swap(0, 1);
        let pushed = apply_container_source(
            &mut s,
            &mut memory,
            Some(two()),
            guids,
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(
            seen(&mut s),
            vec![
                "PLAYERBANKSLOTS_CHANGED nil".to_string(),
                "PLAYERBANKSLOTS_CHANGED nil".to_string()
            ],
            "the swap the pushed containers cannot see"
        );
        assert!(pushed, "and the feed reports that it spoke to the VM");
    }

    /// `BAG_CLOSED` (`0x4f92b5`): a swap fires it, an unequip fires it without `BAG_UPDATE`, and
    /// two identical bags swapped still close both windows.
    #[test]
    fn bag_closed_fires_on_the_bag_guid_moving_and_only_off_a_non_empty_slot() {
        let mut s = UiScript::new().unwrap();
        s.run(
            "SEEN = {} \
             local f = CreateFrame('Frame') \
             f:RegisterEvent('BAG_CLOSED') \
             f:RegisterEvent('BAG_UPDATE') \
             f:SetScript('OnEvent', function() \
                 table.insert(SEEN, event .. ' ' .. tostring(arg1)) end)",
        )
        .unwrap();
        let seen = |s: &mut UiScript| {
            let out = s.eval::<Vec<String>>("return SEEN").unwrap();
            s.run("SEEN = {}").unwrap();
            out
        };
        let pouch = |n: u32| {
            HashMap::from([(
                1,
                ContainerState {
                    name: Some("Small Brown Pouch".into()),
                    num_slots: n,
                    slots: HashMap::new(),
                },
            )])
        };
        let mut guids = NO_BAGS;
        let mut memory = FeedMemory::default();

        // Equipped into an empty slot: `BAG_UPDATE` only (`0x4f9247`).
        guids.bags[0] = 0xAAAA;
        apply_container_source(
            &mut s,
            &mut memory,
            Some(pouch(6)),
            guids,
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(seen(&mut s), vec!["BAG_UPDATE 1"]);

        apply_container_source(
            &mut s,
            &mut memory,
            Some(pouch(6)),
            guids,
            Vec::new(),
            Vec::new(),
        );
        assert!(
            seen(&mut s).is_empty(),
            "an unchanged container says nothing"
        );

        // Swapped for another bag: closed, then updated.
        guids.bags[0] = 0xBBBB;
        apply_container_source(
            &mut s,
            &mut memory,
            Some(pouch(10)),
            guids,
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(seen(&mut s), vec!["BAG_CLOSED 1", "BAG_UPDATE 1"]);

        // Unequipped: closed, and no `BAG_UPDATE`.
        guids.bags[0] = 0;
        apply_container_source(
            &mut s,
            &mut memory,
            Some(HashMap::new()),
            guids,
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(seen(&mut s), vec!["BAG_CLOSED 1"]);

        // Two identical empty bags swapped between containers 5 and 6: equal states, both close.
        let mut memory = FeedMemory::default();
        let two = || {
            HashMap::from([
                (
                    5,
                    ContainerState {
                        name: Some("Linen Bag".into()),
                        num_slots: 6,
                        slots: HashMap::new(),
                    },
                ),
                (
                    6,
                    ContainerState {
                        name: Some("Linen Bag".into()),
                        num_slots: 6,
                        slots: HashMap::new(),
                    },
                ),
            ])
        };
        let mut guids = NO_BAGS;
        guids.bags[4] = 0x1111;
        guids.bags[5] = 0x2222;
        apply_container_source(
            &mut s,
            &mut memory,
            Some(two()),
            guids,
            Vec::new(),
            Vec::new(),
        );
        let _ = seen(&mut s);
        guids.bags.swap(4, 5);
        let pushed = apply_container_source(
            &mut s,
            &mut memory,
            Some(two()),
            guids,
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(seen(&mut s), vec!["BAG_CLOSED 5", "BAG_CLOSED 6"]);
        assert!(
            pushed,
            "a BAG_CLOSED with no container change still went into the VM — the gate audit reads \
             this return"
        );
    }
    use benilla_protocol::messages::ItemSpellEntry;
    use benilla_ui::script::{ContainerState, UiScript};
    use std::collections::HashMap;

    /// An absent self store is no source, never an empty bag burst; a present source that lost a
    /// bag still announces.
    #[test]
    fn an_absent_self_player_is_no_source_never_an_empty_bag_burst() {
        let mut s = UiScript::new().unwrap();
        s.run(
            "BAG_EVENTS = 0 \
             local f = CreateFrame('Frame') \
             f:RegisterEvent('BAG_UPDATE') \
             f:SetScript('OnEvent', function() BAG_EVENTS = BAG_EVENTS + 1 end)",
        )
        .unwrap();
        let bag0 = ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots: HashMap::new(),
        };
        let mut memory = FeedMemory::default();

        // In session: a new bag is pushed and announced.
        apply_container_source(
            &mut s,
            &mut memory,
            Some(HashMap::from([(0, bag0)])),
            NO_BAGS,
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(s.eval::<i64>("return GetContainerNumSlots(0)").unwrap(), 16);
        assert_eq!(s.eval::<i64>("return BAG_EVENTS").unwrap(), 1);

        // The logout despawn frame: the VM keeps its bags and nothing fires.
        apply_container_source(&mut s, &mut memory, None, NO_BAGS, Vec::new(), Vec::new());
        assert_eq!(
            s.eval::<i64>("return GetContainerNumSlots(0)").unwrap(),
            16,
            "an absent source must not empty the VM's containers"
        );
        assert_eq!(
            s.eval::<i64>("return BAG_EVENTS").unwrap(),
            1,
            "an absent source must not fire a BAG_UPDATE burst"
        );

        // A present source without the bag is a real transition.
        apply_container_source(
            &mut s,
            &mut memory,
            Some(HashMap::new()),
            NO_BAGS,
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(s.eval::<i64>("return GetContainerNumSlots(0)").unwrap(), 0);
        assert_eq!(s.eval::<i64>("return BAG_EVENTS").unwrap(), 2);
    }

    /// A stack ticking down fires an argless `ITEM_LOCK_CHANGED` before its bag's `BAG_UPDATE`
    /// (`0x5d94b9`): an auto-shot timer such as Quiver's learns of a shot from it alone. A changed
    /// entry is a swap and must not fire it, or the timer counts shots never fired.
    #[test]
    fn a_stack_ticking_down_fires_item_lock_changed_before_bag_update() {
        use benilla_ui::script::ContainerSlot;

        let mut s = UiScript::new().unwrap();
        s.run(
            "ORDER = {} \
             local f = CreateFrame('Frame') \
             f:RegisterEvent('ITEM_LOCK_CHANGED') \
             f:RegisterEvent('BAG_UPDATE') \
             f:SetScript('OnEvent', function() \
                 table.insert(ORDER, event .. ':' .. tostring(arg1) .. ',' .. tostring(arg2)) \
             end)",
        )
        .unwrap();

        let arrows = |count: u32| ContainerSlot {
            item_id: 2512, // Rough Arrow
            count,
            ..Default::default()
        };
        let bag = |s: ContainerSlot| {
            HashMap::from([(
                0,
                ContainerState {
                    name: Some("Backpack".into()),
                    num_slots: 16,
                    slots: HashMap::from([(1, s)]),
                },
            )])
        };
        let mut memory = FeedMemory::default();

        // The quiver arrives full: a create, no lock event.
        apply_container_source(
            &mut s,
            &mut memory,
            Some(bag(arrows(200))),
            NO_BAGS,
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(
            s.eval::<String>("return table.concat(ORDER, ' ')").unwrap(),
            "BAG_UPDATE:0,nil",
            "a slot appearing is a create, not a stack-count field write"
        );

        // An arrow leaves the quiver.
        s.run("ORDER = {}").unwrap();
        apply_container_source(
            &mut s,
            &mut memory,
            Some(bag(arrows(199))),
            NO_BAGS,
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(
            s.eval::<String>("return table.concat(ORDER, ' ')").unwrap(),
            "ITEM_LOCK_CHANGED:nil,nil BAG_UPDATE:0,nil",
            "the shot signal fires — with NO arguments, as all five of the reference's fire sites \
             do — and precedes BAG_UPDATE. The slot it names travels in the fact \
             that it fired at all, which is enough: every consumer repaints from its own `this`, \
             and Quiver's shot timer only needs to know that A shot happened"
        );

        // A different item in the slot, with a different count: a swap, no lock event.
        s.run("ORDER = {}").unwrap();
        let mut other = arrows(20);
        other.item_id = 3033; // Razor Arrow
        apply_container_source(
            &mut s,
            &mut memory,
            Some(bag(other)),
            NO_BAGS,
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(
            s.eval::<String>("return table.concat(ORDER, ' ')").unwrap(),
            "BAG_UPDATE:0,nil",
            "a changed entry is a swap, not a stack-count write"
        );
    }

    fn slot(spell_id: u32, charges: i32) -> ItemSpellEntry {
        ItemSpellEntry {
            index: 0,
            spell_id,
            trigger: 0,
            charges,
            cooldown_ms: -1,
            category: 0,
            category_cooldown_ms: -1,
        }
    }

    /// The builder's charge gate (`0x52da01`, `0x52db51`).
    #[test]
    fn charge_gate_matches_the_real_builder() {
        // Food and water carry `-1` in vmangos `item_template` (bread 4540, water 159 and 5350).
        assert_eq!(charges_count(&[slot(433, -1)]), 0, "food's -1 = no line");
        // A pool that uses the item up: Flame Deflector 4376 (`4057, -5`).
        assert_eq!(charges_count(&[slot(4057, -5)]), 5, "wand-style pool");
        assert_eq!(charges_count(&[slot(4057, 3)]), 3);
        assert_eq!(charges_count(&[slot(433, 0)]), 0, "template 0 = sentinel");
        assert_eq!(charges_count(&[slot(0, -5)]), 0, "no spell = no slot");
        assert_eq!(charges_count(&[]), 0);
        // A leading sentinel slot does not mask a later pool.
        assert_eq!(charges_count(&[slot(433, -1), slot(4057, -10)]), 10);
    }

    /// On the real Spell.dbc, an undescribed spell such as a key's `Opening` builds no trigger
    /// line (`0x52da29`-`0x52da31`), and a described one does.
    #[test]
    fn an_undescribed_spell_prints_no_trigger_line_on_real_data() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let spells = crate::ui_action::Spells {
            catalog: benilla_formats::load_spell_catalog(&mut chain).expect("Spell.dbc"),
            forms: benilla_formats::load_shapeshift_forms(&mut chain)
                .expect("SpellShapeshiftForm.dbc"),
            ranges: benilla_formats::load_spell_ranges(&mut chain).expect("SpellRange.dbc"),
            cast_times: benilla_formats::load_spell_cast_times(&mut chain)
                .expect("SpellCastTimes.dbc"),
            durations: benilla_formats::load_spell_durations(&mut chain)
                .expect("SpellDuration.dbc"),
            radii: benilla_formats::load_spell_radii(&mut chain).expect("SpellRadius.dbc"),
        };

        // No string table: Fireball's description reaches no keyed token.
        let no_strings = |_: &str, _: &[i64]| None;

        // The lock chain's Opening and Closing spells, none with a description.
        for id in [3365u32, 3366, 6246, 6247, 6477, 21651] {
            let d = spells.catalog.get(id).expect("a real Spell.dbc row");
            assert!(
                !d.name.is_empty(),
                "spell {id} has a name — which is exactly what must NOT leak into the tooltip"
            );
            assert_eq!(
                super::spell_desc_text(Some(&spells), id, None, &no_strings),
                None,
                "spell {id} ({:?}) has no description, so the reference prints no trigger line",
                d.name
            );
        }

        // The control: Fireball (133), described, still yields its line.
        let fireball = super::spell_desc_text(Some(&spells), 133, None, &no_strings)
            .expect("a described spell still yields its line");
        assert!(
            fireball.contains("damage"),
            "expected the substituted Fireball description, got {fireball:?}"
        );
    }
}
