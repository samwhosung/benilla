//! The per-frame feeds into the VM: the container snapshots built from the player's descriptor,
//! and the item-tooltip views every window reads.

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;

use benilla_protocol::ItemInfo;
use benilla_ui::script::{ContainerSlot, ContainerState, ScriptValue, UiScript};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::{NetCommands, ObjectStore, Objects, SelfPlayer};
use crate::query_cache::Ask;
use crate::ui_script::gate;

use super::equip_error::equip_error_key;
use super::{
    find_equip_slot, has_key, keyring_size, EquipErrors, BAGS, BAG_SLOT_FIRST, BANK_BAGS,
    BANK_BAG_ID_FIRST, BANK_BAG_SLOT_FIRST, BANK_CONTAINER, BANK_SLOTS, KEYRING_CONTAINER,
    KEYRING_SLOTS, PACK_SLOTS,
};

/// The doll's equipment slots, 0-based descriptor slots 0..=18 (live-API ids 1..=19).
const WORN_SLOTS: u8 = 19;

/// What the container feed last pushed, for per-bag change events.
#[derive(Default)]
pub(crate) struct FeedMemory {
    pushed: HashMap<i64, ContainerState>,
    /// The last `HasKey()` pushed, so its flip is logged once.
    had_key: bool,
    /// The worn items' cooldowns last pushed, by live-API slot id.
    worn_cooldowns: HashMap<usize, (i64, u32, bool)>,
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
    /// The `BAG_UPDATE`s fired since the feed last counted them into [`BagUpdates`].
    bag_updates: u64,
}

/// How many `BAG_UPDATE`s the bag feed has signalled. Each is a `RaiseBagUpdate 0x4f9370`, which
/// first runs the action bar's usable refresh `0x4e5c00` (`0x4f9374`), so the action feed refreshes
/// on its edge.
#[derive(Resource, Default)]
pub(crate) struct BagUpdates(pub(crate) u64);

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
    // The player's skill in a spell's line, the `$`-tokens' level (`TokenContext::skill`).
    skill: &dyn Fn(u32) -> u32,
    home_area: Option<&str>,
    // The active player's `UNIT_FIELD_BYTES_0` byte 2, the `$g`/`$G` branch's input (`0x508214`).
    gender: &dyn Fn() -> u8,
    mods: Option<&crate::spell::SpellModifiers>,
    // The VM's strings for the keyed `$d`/`$s` tokens.
    global: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    let sp = spells?;
    let d = sp.catalog.get(id)?;
    match d.description.as_deref().filter(|t| !t.is_empty()) {
        // The builder tests the expanded text, which can be empty from a non-empty template.
        Some(desc) => {
            let ctx = benilla_formats::TokenContext {
                durations: &sp.durations,
                radii: &sp.radii,
                ranges: Some(&sp.ranges),
                skill,
                lookup: &|i| sp.catalog.get(i),
                mods: mods.map(|m| m as &dyn benilla_formats::SpellMods),
                unmodified_points: false,
                gender,
                home_area: &|| home_area,
                global,
                printf: &crate::ui_script::token_printf,
            };
            Some(benilla_formats::substitute(desc, d, &ctx)).filter(|t| !t.is_empty())
        }
        None => None,
    }
}

/// What the `$`-tokens read of the player beside the modifiers: the skills the per-level terms
/// scale by, and the gender byte `$g`/`$G` branch on (`0x508214`).
type TokenReads = (crate::spell::SkillSnapshot, u8);

fn token_reads(me: Option<&ObjectStore>) -> TokenReads {
    let gender = me.and_then(|s| s.0.unit_gender()).unwrap_or(0);
    (crate::spell::skill_snapshot(me), gender)
}

/// Only descriptions with substitution tokens can change when the caster's spell mods change.
fn spell_has_mod_tokens(spells: Option<&crate::ui_action::Spells>, id: u32) -> bool {
    spells
        .and_then(|s| s.catalog.get(id))
        .and_then(|d| d.description.as_deref())
        .is_some_and(|desc| desc.contains('$'))
}

fn item_has_mod_tokens(t: &ItemInfo, spells: Option<&crate::ui_action::Spells>) -> bool {
    t.spells
        .iter()
        .any(|slot| slot.spell_id != 0 && spell_has_mod_tokens(spells, slot.spell_id))
}

fn bonuses_have_mod_tokens(
    bonuses: &[(u32, u32)],
    spells: Option<&crate::ui_action::Spells>,
) -> bool {
    bonuses
        .iter()
        .any(|&(_, spell)| spell_has_mod_tokens(spells, spell))
}

/// The static tables a template view names its ids through, all loaded at startup.
#[derive(Clone, Copy, Default)]
struct ViewTables<'a> {
    skill_lines: Option<&'a benilla_formats::SkillLineCatalog>,
    factions: Option<&'a benilla_formats::FactionCatalog>,
    sub_classes: Option<&'a benilla_formats::ItemSubClassCatalog>,
    classes: Option<&'a benilla_formats::ItemClassCatalog>,
    icons: Option<&'a ItemDisplays>,
    // `AreaTable.dbc` and `Map.dbc` names, for a zone-bound item's lines.
    areas: Option<&'a benilla_formats::QuestHeaderNames>,
    maps: Option<&'a benilla_formats::MapCatalog>,
}

/// An item template's tooltip view: the wire fields plus the strings only the app resolves, the
/// trigger-spell text and the skill, spell and reputation requirements.
fn template_view(
    t: &ItemInfo,
    spells: Option<&crate::ui_action::Spells>,
    mods: Option<&crate::spell::SpellModifiers>,
    skill: &dyn Fn(u32) -> u32,
    tables: ViewTables,
    home_area: Option<&str>,
    // The active player's gender byte, the trigger text's `$g`/`$G` branch (`0x508214`) and the
    // standing label's (`0x612bf0`).
    gender: &dyn Fn() -> u8,
    // The VM's own `GlobalStrings.lua`, for the reputation-requirement line and the `$`-engine's
    // keyed tokens.
    get: &dyn Fn(&str) -> Option<String>,
) -> benilla_ui::script::ItemTemplateView {
    let ViewTables {
        skill_lines,
        factions,
        sub_classes,
        classes,
        icons,
        areas,
        maps,
    } = tables;
    let spell_name = |id: u32| -> Option<String> {
        spells
            .and_then(|s| s.catalog.get(id))
            .map(|sd| sd.name.clone())
    };
    let spell_text = |id: u32| spell_desc_text(spells, id, skill, home_area, gender, mods, get);
    benilla_ui::script::ItemTemplateView {
        name: t.name.clone(),
        quality: t.quality,
        class: t.class,
        subclass: t.subclass,
        inventory_type: t.inventory_type,
        proficiency_alt: sub_classes.and_then(|c| c.proficiency_alt(t.class, t.subclass)),
        proficiency_prereq: sub_classes
            .and_then(|c| c.proficiency_prerequisite(t.class, t.subclass)),
        item_level: t.item_level,
        // The five slots' ids that name a spell row, with their charges (`0x4fe890`).
        spell_slots: t
            .spells
            .iter()
            .filter_map(|s| spell_name(s.spell_id).map(|n| (n, s.charges)))
            .collect(),
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
        // Knowing the spell or a later rank of it meets the requirement (`0x60c740`,
        // `0x60c8d0`).
        required_spell_ranks: match (t.required_spell, skill_lines) {
            (0, _) => Vec::new(),
            (id, Some(c)) => c.rank_and_later(id),
            (id, None) => vec![id],
        },
        required_honor_rank: t.required_honor_rank,
        required_city_rank: t.required_city_rank,
        // `ITEM_REQ_REPUTATION`, the reference's key for this line (`0x854b8c`): faction, then
        // standing; an install missing the key shows no line.
        required_rep_line: (t.required_rep_faction != 0)
            .then(|| {
                // A faction with no row reads the literal `UNKNOWN` (`0x838044`, `0x52d800`); the
                // label is gendered (`0x52d80f`) and an absent one is empty.
                let faction = factions
                    .and_then(|c| c.faction_name(t.required_rep_faction))
                    .unwrap_or("UNKNOWN");
                let standing =
                    benilla_ui::strings::standing_label(get, t.required_rep_rank, gender() == 1);
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
        // A block with no description has no lines (`0x52da29`..`0x52da31`).
        spell_lines: t
            .spells
            .iter()
            .filter(|s| s.spell_id != 0)
            .filter_map(|s| {
                Some(benilla_ui::script::ItemSpellLine {
                    trigger: s.trigger,
                    spell_id: s.spell_id,
                    text: spell_text(s.spell_id)?,
                    charges: s.charges,
                    block: s.index,
                    recipe: recipe_of(spells, s.spell_id),
                })
            })
            .collect(),
        description: t.description.clone(),
        page_text: t.page_text,
        sell_price: t.sell_price,
        item_set: t.item_set,
        random_property: t.random_property,
        // Block 0's charges, unfiltered (`+0x144`), the sell price's charge scale (`0x52e432`).
        spell_charges_0: t.spell_charges_0,
        // The area and map a zone-bound item names (`0x52bac9`..`0x52bb3e`): an id above 0 with
        // a row.
        area_name: i32::try_from(t.area)
            .ok()
            .filter(|&id| id > 0)
            .and_then(|id| areas?.resolve(id))
            .map(str::to_string),
        map_name: (t.map != 0 && i32::try_from(t.map).is_ok())
            .then(|| maps?.name(t.map))
            .flatten()
            .map(str::to_string),
        teaches: taught_spell(t, spells, skill_lines),
    }
}

/// A block's recipe (`0x52daf5`..`0x52db25`): its spell's Effect[0] is 36, and the spell that
/// teaches (EffectTriggerSpell[0], `+0x1b4`) has Effect[0] 24, `SPELL_EFFECT_CREATE_ITEM`. The
/// product is its EffectItemType[0] (`+0x19c`), the reagents its `Reagent` ids above 0 with their
/// counts (`+0xa8`, `+0xc8`, `0x52dbf8`).
fn recipe_of(
    spells: Option<&crate::ui_action::Spells>,
    spell_id: u32,
) -> Option<benilla_ui::script::ItemRecipe> {
    let catalog = &spells?.catalog;
    let d = catalog.get(spell_id).filter(|d| d.effects[0] == 36)?;
    let taught = catalog
        .get(d.effect_trigger_spell[0])
        .filter(|t| t.effects[0] == 24)?;
    Some(benilla_ui::script::ItemRecipe {
        product: taught.effect_item_type[0],
        reagents: taught
            .reagents
            .iter()
            .filter(|&&(id, _)| id as i32 > 0)
            .map(|&(id, count)| (id, count as i32))
            .collect(),
    })
}

/// The spell block 0 teaches (`0x52d4fd`..`0x52d54d`): its spell's Effect[0] is 36
/// (`SPELL_EFFECT_LEARN_SPELL`, `+0xf4`), the taught spell its EffectTriggerSpell[0] (`+0x1b4`),
/// with every later rank of it, and a pet teach when either implicit target of effect 0 is 5
/// (`TARGET_PET`, `+0x148`, `+0x154`).
fn taught_spell(
    t: &ItemInfo,
    spells: Option<&crate::ui_action::Spells>,
    skill_lines: Option<&benilla_formats::SkillLineCatalog>,
) -> Option<benilla_ui::script::TaughtSpell> {
    let block = t.spells.iter().find(|s| s.index == 0)?;
    let d = spells?.catalog.get(block.spell_id)?;
    (d.effects[0] == 36).then(|| {
        let taught = d.effect_trigger_spell[0];
        benilla_ui::script::TaughtSpell {
            ranks: skill_lines.map_or_else(|| vec![taught], |c| c.rank_and_later(taught)),
            pet: d.effect_implicit_target_a[0] == 5 || d.effect_implicit_target_b[0] == 5,
        }
    })
}

/// Answers the engine's set-id asks from `ItemSet.dbc`: member names from the template cache, each
/// miss queried as the reference queries set members, and bonus text from the `$`-engine. A set
/// stays pending, re-pushing, until every member resolves.
pub(super) fn feed_item_sets(
    mut hosted_vm: crate::script_calls::HostedVm,
    sets: Option<Res<super::ItemSets>>,
    items: Res<Items>,
    commands: Res<NetCommands>,
    spells: Option<Res<crate::ui_action::Spells>>,
    spell_mods: Res<crate::spell::SpellModifiers>,
    skill_lines: Option<Res<crate::ui_spellbook::SkillLines>>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    mut pending: Local<
        crate::ui_script::VmMemo<std::collections::HashMap<u32, benilla_ui::script::ItemSetView>>,
    >,
    mut mod_sensitive: Local<crate::ui_script::VmMemo<HashSet<u32>>>,
    mut last_reads: Local<crate::ui_script::VmMemo<Option<TokenReads>>>,
) {
    hosted_vm.run(|script| {
        let Some(script) = script else {
            return;
        };
        let pending = pending.get(script);
        let mod_sensitive = mod_sensitive.get(script);
        let me = self_q.single().ok();
        // The build's asks are its lookups with a callback (`0x52bf60`): their members are asked,
        // a refused one again; a set carried since only reads.
        let asked: HashSet<u32> = script.take_item_set_asks().into_iter().collect();
        for &id in &asked {
            pending.entry(id).or_default();
        }
        // The `$`-tokens follow the player's skills and gender.
        let reads = token_reads(me);
        let gender = reads.1;
        let reads_changed = last_reads.get(script).as_ref() != Some(&reads);
        *last_reads.get(script) = Some(reads);
        if spell_mods.is_changed() || reads_changed {
            for &id in mod_sensitive.iter() {
                pending.entry(id).or_default();
            }
        }
        if pending.is_empty() {
            return;
        }
        let Some(sets) = sets.as_deref() else {
            return; // no catalog without client data: the asks stay parked
        };
        let spell_res = spells.as_deref();
        let skill_catalog = skill_lines.as_deref().map(|s| &s.catalog);
        let skill = |id| crate::spell::spell_skill_value(me, skill_catalog, id);
        let mut done: Vec<u32> = Vec::new();
        let mut push: Vec<(u32, benilla_ui::script::ItemSetView)> = Vec::new();
        // Its borrow of the VM ends before the push, which needs the VM mutably.
        let global = |key: &str| benilla_ui::strings::global(script.lua(), key);
        for (&set_id, last) in pending.iter_mut() {
            let Some(row) = sets.0.set(set_id) else {
                done.push(set_id); // no such row: drop the ask
                continue;
            };
            if bonuses_have_mod_tokens(&row.bonuses, spell_res) {
                mod_sensitive.insert(set_id);
            } else {
                mod_sensitive.remove(&set_id);
            }
            let ask = if asked.contains(&set_id) {
                Ask::Event(&commands)
            } else {
                Ask::Read
            };
            let members: Vec<(u32, Option<String>)> = row
                .items
                .iter()
                .map(|&id| (id, items.template(id, 0, ask).map(|t| t.name.clone())))
                .collect();
            let view = benilla_ui::script::ItemSetView {
                name: row.name.clone(),
                bonuses: row
                    .bonuses
                    .iter()
                    .filter_map(|&(n, spell)| {
                        spell_desc_text(
                            spell_res,
                            spell,
                            &skill,
                            None,
                            &|| gender,
                            Some(&spell_mods),
                            &global,
                        )
                        .map(|desc| (n, desc))
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
        for (id, view) in push {
            script.set_item_set(id, view);
        }
        for id in done {
            pending.remove(&id);
        }
    })
}

/// Pushes the whole random-suffix table once per VM, when both catalogs are in, and the enchant
/// names once the enchant catalog is: their consumers are click-driven, and a chat-link tooltip
/// has no hover loop to repaint a late answer.
pub(super) fn feed_random_properties(
    mut hosted_vm: crate::script_calls::HostedVm,
    props: Option<Res<crate::items::RandomProperties>>,
    enchants: Option<Res<crate::items::Enchants>>,
    mut pushed: Local<crate::ui_script::VmMemo<bool>>,
    mut names_pushed: Local<crate::ui_script::VmMemo<bool>>,
) {
    hosted_vm.run(|script| {
        let Some(script) = script else {
            return;
        };
        // The enchant names a link's bare id reads (`0x52c9f2`): every named row, as the builder's
        // loop tests no flag before it prints the name (`0x52ca23`..`0x52ca8b`).
        if let Some(enchants) = enchants.as_deref() {
            if !*names_pushed.get(script) {
                let names: std::collections::HashMap<u32, String> = enchants
                    .0
                    .iter_names()
                    .map(|(id, name)| (id, name.to_string()))
                    .collect();
                script.set_enchant_names(names);
                *names_pushed.get(script) = true;
            }
        }
        if *pushed.get(script) {
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
        *pushed.get(script) = true;
    })
}

/// The item-tooltip feed: every template that lands is pushed unprompted, and a new VM is seated
/// with every cached one, so the first hover never misses, as the reference reads one item cache
/// synchronously that a `/reload` keeps. A miss the VM asked for is the
/// getter's lookup with a callback, which queries it here and asks a refused one again; a miss
/// `GetItemInfo` read is served from the cache and never queried (`0x48e172`).
pub(super) fn feed_item_stats(
    mut vm: crate::script_calls::VmWith<ResMut<'static, Items>>,
    commands: Res<NetCommands>,
    spells: Option<Res<crate::ui_action::Spells>>,
    // The caster the `$`-tokens read: its spell modifiers and its skills.
    (spell_mods, self_q): CasterReads,
    skill_lines: Option<Res<crate::ui_spellbook::SkillLines>>,
    // The `$z` token: the bind point's area (`SMSG_BINDPOINTUPDATE`), named through `AreaTable`.
    home_bind: Option<Res<crate::net::HomeBind>>,
    (area_names, maps): (
        Option<Res<crate::ui_quest_log::QuestHeaderNamesRes>>,
        Option<Res<benilla_assets::MapCatalogRes>>,
    ),
    factions: Option<Res<crate::target::Factions>>,
    sub_classes: Option<Res<super::ItemSubClasses>>,
    // `GetItemInfo`'s `itemType` and `itemTexture`.
    classes: Option<Res<super::ItemClasses>>,
    icons: Option<Res<ItemDisplays>>,
    mut pending: Local<crate::ui_script::VmMemo<std::collections::HashSet<u32>>>,
    // The views built from the player beyond the template, rebuilt when what they read moves.
    mut reads_player: Local<crate::ui_script::VmMemo<HashSet<u32>>>,
    mut last_home: Local<crate::ui_script::VmMemo<Option<String>>>,
    mut last_reads: Local<crate::ui_script::VmMemo<Option<TokenReads>>>,
) {
    vm.turn(|script, mut items| {
        let Some(script) = script else {
            return;
        };
        let (pending, new_vm) = pending.get_reset(script);
        if new_vm {
            pending.extend(items.cached_template_ids());
        }
        let reads_player = reads_player.get(script);
        let last_home = last_home.get(script);
        let me = self_q.single().ok();
        // The `$`-tokens follow the player's skills and gender.
        let reads = token_reads(me);
        let gender = reads.1;
        let reads_changed = last_reads.get(script).as_ref() != Some(&reads);
        *last_reads.get(script) = Some(reads);
        // `GetBindLocation()`'s push, here so it and the `$z` token share one name, and ahead of
        // the pending gate below: the bind point can arrive while the feed idles.
        let home_area: Option<&str> = home_bind
            .as_deref()
            .and_then(|b| b.0)
            .and_then(|id| area_names.as_deref()?.0.resolve(id as i32));
        // Pushed on change, per VM: a rebuilt VM is fed again the frame it appears.
        if last_home.as_deref() != home_area {
            script.set_bind_location(home_area.unwrap_or_default());
            *last_home = home_area.map(str::to_string);
            // Re-substitute every held view, or one pushed before the bind point carries a raw
            // `$z`.
            pending.extend(items.cached_template_ids());
        }

        pending.extend(items.take_fresh());
        for id in script.take_item_stat_asks() {
            items.template(id, 0, Ask::Event(&commands));
            pending.insert(id);
        }
        // A read the cache misses has nothing to push; a later landing pushes it unprompted.
        pending.extend(
            script
                .take_item_stat_reads()
                .into_iter()
                .filter(|&id| items.template(id, 0, Ask::Read).is_some()),
        );
        if spell_mods.is_changed() || reads_changed {
            pending.extend(reads_player.iter().copied());
        }
        if pending.is_empty() {
            return;
        }
        let spell_res = spells.as_deref();
        let skill_catalog = skill_lines.as_deref().map(|s| &s.catalog);
        let skill = |id| crate::spell::spell_skill_value(me, skill_catalog, id);
        let ready: Vec<u32> = pending
            .iter()
            .copied()
            .filter(|&id| items.template(id, 0, Ask::Read).is_some())
            .collect();
        // Built first, pushed after: the lookup borrows the VM that `set_item_template` needs
        // mutably.
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
                    let t = items.template(id, 0, Ask::Read)?.clone();
                    // The standing label reads the gender too.
                    if item_has_mod_tokens(&t, spell_res) || t.required_rep_faction != 0 {
                        reads_player.insert(id);
                    } else {
                        reads_player.remove(&id);
                    }
                    let tables = ViewTables {
                        skill_lines: skill_catalog,
                        factions: factions.as_deref().map(|f| f.catalog()),
                        sub_classes: sub_classes.as_deref().map(|s| &s.0),
                        classes: classes.as_deref().map(|c| &c.0),
                        icons: icons.as_deref(),
                        areas: area_names.as_deref().map(|a| &a.0),
                        maps: maps.as_deref().map(|m| &m.0),
                    };
                    let view = template_view(
                        &t,
                        spell_res,
                        Some(&spell_mods),
                        &skill,
                        tables,
                        home_area,
                        &|| gender,
                        &get,
                    );
                    Some((id, view))
                })
                .collect()
        };
        for (id, view) in views {
            script.set_item_template(id, view);
        }
    })
}

/// What the `$`-tokens read of the caster.
type CasterReads<'w, 's> = (
    Res<'w, crate::spell::SpellModifiers>,
    Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
);

/// The player state the tooltip's red requirement lines check: level, class, race, skills,
/// proficiencies (`SMSG_SET_PROFICIENCY`), reputation ranks (the DBC base plus the
/// `SMSG_INITIALIZE_FACTIONS` standing), honor rank and medals, the known spells, and the
/// skinning value the unit tooltip colours by, pushed on change.
pub(super) fn feed_player_req(
    mut hosted_vm: crate::script_calls::HostedVm,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    changed_self: Query<(), (With<SelfPlayer>, Changed<ObjectStore>)>,
    proficiencies: Res<crate::net::Proficiencies>,
    reputations: Res<crate::net::Reputations>,
    factions: Option<Res<crate::target::Factions>>,
    actions: Res<crate::ui_action::PlayerActions>,
    spells: Option<Res<crate::ui_action::Spells>>,
    // The skinning latch `[0xb700e4]` and the line its value scales by.
    skinning: (
        Res<crate::ui_action::LearnedAbilities>,
        Option<Res<crate::ui_spellbook::SkillLines>>,
    ),
    mut last: Local<crate::ui_script::VmMemo<Option<benilla_ui::script::PlayerReqState>>>,
) {
    hosted_vm.run(|script| {
        let (learned, skill_lines) = skinning;
        let Some(script) = script else {
            return;
        };
        let (last, vm_reset) = last.get_reset(script);
        // The state is a function of these inputs alone.
        let self_changed = !changed_self.is_empty();
        let prof_changed = proficiencies.is_changed();
        let reps_changed = reputations.is_changed();
        let factions_changed = factions.as_ref().is_some_and(|r| r.is_changed());
        let actions_changed = actions.is_changed();
        let spells_changed = spells.as_ref().is_some_and(|r| r.is_changed());
        let learned_changed = learned.is_changed();
        let lines_changed = skill_lines.as_ref().is_some_and(|r| r.is_changed());
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
                ("learned", learned_changed),
                ("skill_lines", lines_changed),
            ],
        );
        let gate = gate::Gate::new(
            vm_reset
                || self_changed
                || prof_changed
                || reps_changed
                || factions_changed
                || actions_changed
                || spells_changed
                || learned_changed
                || lines_changed,
        );
        if gate.skip() {
            return;
        }
        // The known-spell set the item tooltip's known-recipe line tests (`0x60c740`), the whole
        // book: a recipe's taught craft is a spell the spellbook does not list.
        if vm_reset || actions_changed {
            script.set_known_spells(actions.spells.iter().copied().collect());
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
                    .and_then(|i| reputations.slots.get(i))
                    .map(|&(_, s)| s)
                    .unwrap_or(0);
                rep_ranks.insert(
                    id,
                    benilla_formats::reputation_rank(info.base_for(race, class) + standing),
                );
            }
        }
        // The reference's `0xc4d770`: a known spell whose Effect[0] is 40
        // (`SPELL_EFFECT_DUAL_WIELD`).
        let can_dual_wield = spells.as_deref().is_some_and(|s| {
            actions
                .spells
                .iter()
                .any(|&id| s.catalog.get(id).is_some_and(|sd| sd.effects[0] == 40))
        });
        // The skinnable line's value (`0x529f06`-`0x529f58`): the latched spell's first `SKINNING`
        // effect, its points at the player's skill in the spell's line, kept only above 0.
        let skinning = learned
            .skinning
            .and_then(|id| {
                let d = spells.as_deref()?.catalog.get(id)?;
                let e = d
                    .effects
                    .iter()
                    .position(|&e| e == benilla_formats::SPELL_EFFECT_SKINNING)?;
                let catalog = skill_lines.as_deref().map(|s| &s.catalog);
                let skill = crate::spell::spell_skill_value(Some(store), catalog, id);
                Some(d.effect_points_int(e, skill))
            })
            .filter(|&v| v > 0);
        let state = benilla_ui::script::PlayerReqState {
            level: store.0.unit_level().unwrap_or(0),
            class_id: u32::from(class),
            race_id: u32::from(race),
            skills,
            proficiency: proficiencies.0.clone(),
            rep_ranks,
            can_dual_wield,
            honor_rank: store.0.player_honor_rank().unwrap_or(0),
            pvp_medals: store.0.player_pvp_medals().unwrap_or(0),
            skinning,
        };
        if last.as_ref() != Some(&state) {
            gate.audit("feed_player_req", "the player-requirement state");
            *last = Some(state.clone());
            script.set_player_req_state(state);
        }
    })
}

/// One bag slot from its item guid: instance, template, icon and use-spell cooldown. `None` is an
/// empty slot or one whose item object has not streamed; an occupied one whose template is not yet
/// resolved is `Some` with empty fields.
fn resolve_slot(
    guid: u64,
    objects: &Objects,
    items: &Items,
    icons: Option<&ItemDisplays>,
    rolls: crate::items::RollCatalogs,
    cooldowns: &crate::spell::Cooldowns,
    spells: Option<&benilla_formats::SpellCatalog>,
    names: &crate::names::NameCache,
    // Mutable: reading a charter's record is what queries it.
    petitions: &crate::ui_petition::PetitionState,
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
    // A guid whose create has not landed: the reference finds no item object (`0x6228a0` returns
    // NULL), so `GetContainerItemInfo` pushes nothing (`0x4f96dc`, `0x4f96e0`, `0x4f9810`) and
    // the slot reads empty.
    let fields = objects.object(guid)?;
    let (entry, count, durability, readable, creator, flags, already_bound, roll, enchant_lines) = (
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
        // name query answers. The tooltip build asks it ([`ask_item_build_lookups`]).
        fields
            .item_creator()
            .filter(|&g| g != 0)
            .and_then(|g| names.resolve_player(g, Ask::Read).map(str::to_string)),
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
    );
    let gift_creator = crate::items::gift_creator_name(fields, names);
    let (creator_guid, gift_creator_guid) = crate::items::creator_guids(fields);
    // A container's filled seats, which `ClickTradeButton` counts (`0x4bff50`).
    let occupied_seats = fields.is_container().then(|| {
        let n = fields.container_num_slots().unwrap_or(0).min(36) as u8;
        (0..n)
            .filter(|&j| fields.container_slot(j).is_some_and(|g| g != 0))
            .count() as u32
    });
    // The guid rides every shape, so the slot's lock reads from its first push.
    if entry == 0 {
        return Some(ContainerSlot {
            guid,
            ..Default::default()
        });
    }
    // Only read, as `GetContainerItemInfo` reads it (`0x4f978b`): the item's create asked.
    let template: Option<ItemInfo> = items.template(entry, guid, Ask::Read).cloned();
    let Some(t) = template else {
        // Asked (or a cached negative); show the slot occupied while the answer is in flight.
        return Some(ContainerSlot {
            durability: None,
            guid,
            item_id: entry,
            count,
            readable,
            creator,
            gift_creator,
            creator_guid,
            gift_creator_guid,
            flags,
            already_bound,
            occupied_seats,
            enchants: enchant_lines,
            duration_ms,
            ..Default::default()
        });
    };
    Some(ContainerSlot {
        use_fork: super::drain::use_fork(&t, flags, fields.item_text_id().unwrap_or(0)),
        item_class: Some(t.class),
        bonding: t.bonding,
        use_spell: t.use_spell.map_or(0, |u| u.spell_id),
        // A charter's guild name and master, keyed by the petition id the server puts in
        // enchantment slot 0 (`PetitionsHandler.cpp:126`), where the reference reads it
        // (`0x5ef337`) and its enchant lines skip it (`0x52c9e0`). The tooltip build asks it.
        // The signature count rides beside the id (`0x52ba51`).
        petition: (t.flags & benilla_protocol::messages::ITEM_FLAG_CHARTER != 0)
            .then(|| {
                let id = fields.item_enchant(0).unwrap_or(0);
                let view = petitions.tooltip_view(id as u32, guid, names, Ask::Read)?;
                Some(benilla_ui::script::PetitionSlotView {
                    signatures: fields.item_petition_signatures(),
                    ..view
                })
            })
            .flatten(),
        texture: icons
            .and_then(|i| i.catalog.get(t.display_info_id))
            .and_then(|d| d.icon.clone()),
        count,
        durability,
        quality: Some(t.quality),
        inventory_type: t.inventory_type,
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
        guid,
        readable,
        creator,
        gift_creator,
        creator_guid,
        gift_creator_guid,
        flags,
        already_bound,
        occupied_seats,
        enchants: enchant_lines,
        duration_ms,
        equip_slots: find_equip_slot(t.inventory_type, has_relic_slot),
        bar_placeable: t.placeable_on_action_bar(),
        consumable: t.is_consumable(),
        spell_0: t.has_spell_0(),
        // The tooltip's charges lines (`0x52d9e9`) and the sell price's charge scale
        // (`0x52e43d`) read them (`flds+0x28`).
        spell_charges: std::array::from_fn(|i| fields.item_spell_charges(i as u8).unwrap_or(0)),
        cooldown: use_cooldown(
            &t,
            entry,
            fields.item_spell_charges(0).unwrap_or(0),
            cooldowns,
            spells,
            now,
            ui_now,
        ),
    })
}

/// What the reference's item query `0x6e2ed0` hands the getters and tooltip setters, `(start_ms on
/// the GetTime clock, duration_ms, enabled)`, for the item copy holding `charges_left`
/// (`ITEM_FIELD_SPELL_CHARGES[0]`): by the entry and its first on-use spell, wherever the copy
/// sits.
///
/// `None` is the all-zero answer, enable included: a copy with no charge left on a charged
/// template, which skips the query (`0x5347e3`, `0x53318a`, `0x4f9a48`, `0x4c8b55`), and a
/// template with no on-use spell, which the query leaves untouched (`0x6e2f34`). A cold on-use
/// spell answers enabled (`0x6e1406`).
fn use_cooldown(
    t: &ItemInfo,
    entry: u32,
    charges_left: i32,
    cooldowns: &crate::spell::Cooldowns,
    spells: Option<&benilla_formats::SpellCatalog>,
    now: std::time::Instant,
    ui_now: f64,
) -> Option<(i64, u32, bool)> {
    if t.charges_depleted(charges_left) {
        return None;
    }
    let u = t.use_spell?;
    let sd = spells.and_then(|s| s.get(u.spell_id));
    Some(
        cooldowns
            .info_item(u.spell_id, entry, u.category, sd, now)
            .ui_triple(now, ui_now)
            .unwrap_or((0, 0, true)),
    )
}

/// The item query's answer for each worn item by live-API slot id (the descriptor slot plus one),
/// for the tooltip's ITEM_COOLDOWN_TIME line and `GetInventoryItemCooldown`; a slot the query
/// answers zeros for has no entry ([`use_cooldown`]). The doll snapshot does not carry it: a cooldown moves no
/// inventory slot, and that snapshot is pushed on change.
fn worn_cooldowns(
    store: &ObjectStore,
    objects: &Objects,
    items: &Items,
    cooldowns: &crate::spell::Cooldowns,
    spells: Option<&benilla_formats::SpellCatalog>,
    now: std::time::Instant,
    ui_now: f64,
) -> HashMap<usize, (i64, u32, bool)> {
    (0..WORN_SLOTS)
        .filter_map(|slot0| {
            let guid = store.0.player_inv_slot(slot0).filter(|&g| g != 0)?;
            let fields = objects.object(guid)?;
            let entry = fields.object_entry()?;
            // Only read (`0x4c8b3a`).
            let t = items.template(entry, guid, Ask::Read)?;
            let charges_left = fields.item_spell_charges(0).unwrap_or(0);
            let triple = use_cooldown(t, entry, charges_left, cooldowns, spells, now, ui_now)?;
            Some((usize::from(slot0) + 1, triple))
        })
        .collect()
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
    // Only read (`0x5da08d`).
    let family = items.template(entry, guid, Ask::Read)?.bag_family;
    families.name(family).map(str::to_string)
}

/// The item build's own lookups, asked as it asks them. The creator's and the giver's names
/// (`0x52e204`, `0x52e29d`) and a charter's record and owner (`0x52b93b`, `0x52b9d2`) pass a
/// callback at every build, and a hovered bag or paper-doll slot builds again each frame
/// (`ContainerFrameItemButton_OnUpdate`, `PaperDollItemSlotButton_OnUpdate` run their `OnEnter`
/// while the tooltip is theirs), so a refused key is asked again about once per answer. The slot
/// feeds only read what lands.
pub(super) fn ask_item_build_lookups(
    mut hosted_vm: crate::script_calls::HostedVm,
    names: Res<crate::names::NameCache>,
    petitions: Res<crate::ui_petition::PetitionState>,
    objects: Objects,
    commands: Res<NetCommands>,
) {
    hosted_vm.run(|script| {
        let Some(script) = script else {
            return;
        };
        // The petition id rides enchantment slot 0 (`0x5ef337`).
        let petition_id = |item| {
            objects
                .object(item)
                .and_then(|f| f.item_enchant(0))
                .unwrap_or(0) as u32
        };
        let asks = (script.take_item_name_asks(), script.take_petition_asks());
        ask_build_lookups(asks, petition_id, &names, &petitions, &commands);
    })
}

/// One run's asks: the build's missed names, then its charters' records and owners.
fn ask_build_lookups(
    (name_asks, petition_asks): (Vec<u64>, Vec<u64>),
    petition_id: impl Fn(u64) -> u32,
    names: &crate::names::NameCache,
    petitions: &crate::ui_petition::PetitionState,
    commands: &NetCommands,
) {
    for guid in name_asks {
        names.resolve_player(guid, Ask::Event(commands));
    }
    for item in petition_asks {
        petitions.tooltip_view(petition_id(item), item, names, Ask::Event(commands));
    }
}

#[allow(clippy::type_complexity)] // the param list is the input set
pub(crate) fn feed_containers(
    mut vm: crate::script_calls::VmWith<Res<'static, crate::spell::Cooldowns>>,
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
    spells: Option<Res<crate::ui_action::Spells>>,
    equip: (ResMut<EquipErrors>, crate::ui_action::MessageSink),
    // Reason 16's `%s` source; absent, every 16 keeps the generic line.
    bag_families: Option<Res<crate::ui_items::ItemBagFamilies>>,
    mut opens: ResMut<super::net::BagOpens>,
    mut bag_updates: ResMut<BagUpdates>,
    names: Res<crate::names::NameCache>,
    clock_and_petitions: (
        Res<crate::ui_script::UiClock>,
        Res<crate::ui_petition::PetitionState>,
    ),
    mut memory: Local<crate::ui_script::VmMemo<FeedMemory>>,
) {
    // The bags, read off the host's cooldowns among the rest; their push signals `BAG_UPDATE`
    // and `BAG_OPEN`, so it runs with the host lent.
    let Some((source, guids, opened, gate)) = vm.turn(|script, cooldowns| {
        let (mut equip_errors, mut sink) = equip;
        let Some(script) = script else {
            // A refusal's unlocks would fire into a later VM's locks; with none up they are
            // dropped, as the release queue's are, and the lines wait.
            for e in &mut equip_errors.0 {
                e.unlocks = [0; 2];
            }
            return None;
        };
        let (clock, petitions) = clock_and_petitions;
        let (memory, vm_reset) = memory.get_reset(script);
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
        let opens_held = !opens.0.is_empty();
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
                ("opens", opens_held),
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
            || opens_held,
        );
        if gate.skip() {
            return None;
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
        let mut unlocks = Vec::new();
        for e in equip_errors.0.drain(..) {
            unlocks.extend(e.unlocks);
            // Reason 16 names the bag's family when the bag resolves, as `0x5ede00` does.
            let subclass_fill = (e.reason == 16)
                .then(|| {
                    bag_family_name(
                        player,
                        e.bag_slot,
                        &inv.objects,
                        &items,
                        bag_families.as_deref().map(|c| &c.0),
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
        crate::ui_action::show_messages(script, &mut sink, "ui_items", refusals);
        // A refusal's unlocks follow its line (`0x496720`, then `0x5e3ac0`, `0x5e3ad3`).
        for guid in unlocks {
            script.unlock_item(guid);
        }
        // Server-opened bags fire `BAG_OPEN(id)` after the slot push below.
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
                if let Some(slot) = resolve_slot(
                    guid,
                    &inv.objects,
                    &items,
                    icons.as_deref(),
                    rolls,
                    &cooldowns,
                    spell_catalog,
                    &names,
                    &petitions,
                    now,
                    ui_now,
                    has_relic_slot,
                ) {
                    slots.insert(u32::from(i) + 1, slot);
                }
            }
            fresh.insert(
                0,
                ContainerState {
                    name: None,
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
                        // `GetBagName` only reads (`0x4fa75e`).
                        items
                            .template(entry, bag_guid, Ask::Read)
                            .map(|t| t.name.clone())
                    })
                    .flatten();
                let mut slots = HashMap::new();
                for (j, &guid) in slot_guids.iter().enumerate() {
                    if let Some(slot) = resolve_slot(
                        guid,
                        &inv.objects,
                        &items,
                        icons.as_deref(),
                        rolls,
                        &cooldowns,
                        spell_catalog,
                        &names,
                        &petitions,
                        now,
                        ui_now,
                        has_relic_slot,
                    ) {
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

            // The bank, container -1 (the vault) and 5..=10 (its bags), fed whether or not the
            // window is open: the descriptor streams at login.
            let mut slots = HashMap::new();
            for i in 0..BANK_SLOTS {
                let guid = store.0.player_bank_slot(i).unwrap_or(0);
                if let Some(slot) = resolve_slot(
                    guid,
                    &inv.objects,
                    &items,
                    icons.as_deref(),
                    rolls,
                    &cooldowns,
                    spell_catalog,
                    &names,
                    &petitions,
                    now,
                    ui_now,
                    has_relic_slot,
                ) {
                    slots.insert(u32::from(i) + 1, slot);
                }
            }
            fresh.insert(
                BANK_CONTAINER,
                ContainerState {
                    name: None,
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
                        // `GetBagName` only reads (`0x4fa75e`).
                        items
                            .template(entry, bag_guid, Ask::Read)
                            .map(|t| t.name.clone())
                    })
                    .flatten();
                let mut slots = HashMap::new();
                for (j, &guid) in slot_guids.iter().enumerate() {
                    if let Some(slot) = resolve_slot(
                        guid,
                        &inv.objects,
                        &items,
                        icons.as_deref(),
                        rolls,
                        &cooldowns,
                        spell_catalog,
                        &names,
                        &petitions,
                        now,
                        ui_now,
                        has_relic_slot,
                    ) {
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
            // `GetKeyRingSize` (`ContainerFrame.lua:773`) reads off `UnitLevel` and the server
            // shares. Slots past the size, which the server refuses, are not fed.
            let size = keyring_size(store.0.unit_level().unwrap_or(1));
            let mut slots = HashMap::new();
            for i in 0..size.min(u32::from(KEYRING_SLOTS)) as u8 {
                let guid = store.0.player_keyring_slot(i).unwrap_or(0);
                if let Some(slot) = resolve_slot(
                    guid,
                    &inv.objects,
                    &items,
                    icons.as_deref(),
                    rolls,
                    &cooldowns,
                    spell_catalog,
                    &names,
                    &petitions,
                    now,
                    ui_now,
                    has_relic_slot,
                ) {
                    slots.insert(u32::from(i) + 1, slot);
                }
            }
            fresh.insert(
                KEYRING_CONTAINER,
                ContainerState {
                    name: None,
                    num_slots: size,
                    slots,
                },
            );

            // `HasKey()`, which decides whether the keyring shows at all, pushed with the
            // containers so it is fresh on every `BAG_UPDATE` frame.
            let key = has_key(&store.0, &inv.objects, &items);
            if key != memory.had_key {
                gate.audit("feed_containers", "the HasKey() flip");
                debug!(
                    "ui_items: HasKey() -> {key} (keyring {})",
                    if key { "shown" } else { "hidden" }
                );
                memory.had_key = key;
            }
            script.set_has_key(key);

            // The worn items' use cooldowns, for the tooltip line; no event rides them.
            let worn = worn_cooldowns(
                store,
                &inv.objects,
                &items,
                &cooldowns,
                spell_catalog,
                now,
                ui_now,
            );
            if worn != memory.worn_cooldowns {
                gate.audit("feed_containers", "the worn items' cooldowns");
                script.set_worn_cooldowns(worn.clone());
                memory.worn_cooldowns = worn;
            }
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
        Some((
            player.is_some().then_some(fresh),
            SlotGuids {
                bags: std::array::from_fn(|i| player.map_or(0, |s| bag_slot_guid(s, i as i64 + 1))),
                vault: std::array::from_fn(|i| player.map_or(0, |s| vault_slot_guid(s, i as u8))),
            },
            opened,
            gate,
        ))
    }) else {
        return;
    };
    vm.run(|script| {
        let Some(script) = script else {
            return;
        };
        let memory = memory.get(script);
        if apply_container_source(script, memory, source, guids, opened) {
            gate.audit("feed_containers", "a bag diff or a server-opened bag");
        }
        bag_updates.0 = bag_updates
            .0
            .wrapping_add(std::mem::take(&mut memory.bag_updates));
    })
}

/// Diffs `source` against the last push, pushes each changed bag and fires the reference's events:
/// `BAG_UPDATE(bagID)` (`PLAYERBANKSLOTS_CHANGED` for the vault), then `BAG_OPEN`.
///
/// `None` is an absent self store (before login, or the logout despawn frames before the VM shuts
/// down), never "no items": the VM keeps its last push, as the reference's UI shutdown
/// (`0x490bd0`) runs with the inventory intact, where a burst of size-0 `BAG_UPDATE`s would have a
/// bag-saving addon record empty bags. Server-opened bags still flush, never to fire into a later
/// VM.
pub(crate) fn apply_container_source(
    script: &mut UiScript,
    memory: &mut FeedMemory,
    source: Option<HashMap<i64, ContainerState>>,
    // Beside the source, since an absent source must leave the guid memory alone too.
    guids: SlotGuids,
    opened: Vec<i64>,
) -> bool {
    let mut pushed = false;
    if let Some(fresh) = source {
        pushed |= diff_and_push(script, memory, fresh, guids);
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
        // A stack-count write's `ITEM_LOCK_CHANGED` (the entry walk's `0x5d94b9`, or the lone
        // unlock's `0x5d94ca`) fired ahead of this push, from
        // [`crate::ui_items::locks::apply_item_releases`].
        for &bag in &changed {
            script.set_container(bag, fresh.get(&bag).cloned());
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
                memory.bag_updates += 1;
            }
        }
        memory.pushed = fresh;
    }
    // One event per changed vault slot, with no slot argument: every bank button repaints from
    // its own slot (`feed_char` fires the same event for the bank-bag buttons).
    for &same_item in &vault {
        if same_item {
            script.fire_event(
                "PLAYERBANKSLOTS_CHANGED",
                vec![ScriptValue::Str("player".into())],
            );
        } else {
            script.fire_event("PLAYERBANKSLOTS_CHANGED", vec![]);
        }
    }
    // A `BAG_CLOSED` or vault event alone still went into the VM.
    !changed.is_empty() || !closed.is_empty() || !vault.is_empty()
}

#[cfg(test)]
mod tests {
    use super::{
        apply_container_source, ask_build_lookups, resolve_slot, ContainerSlot, FeedMemory,
        SlotGuids, BANK_CONTAINER, BANK_SLOTS,
    };

    /// The view carries what `GetTradeSkillItemStats` reads beyond the tooltip: the item level, and
    /// the five spell slots whose id names a `Spell.dbc` row with their charges, in slot order.
    #[test]
    fn a_template_view_carries_the_item_level_and_the_named_spell_slots() {
        use crate::items::test_template;
        use benilla_formats::SpellDisplay;
        use benilla_protocol::messages::ItemSpellEntry;
        use std::collections::HashMap;

        let mut t = test_template("Wand");
        t.item_level = 33;
        let slot = |index, spell_id, charges| ItemSpellEntry {
            index,
            spell_id,
            trigger: 0,
            charges,
            cooldown_ms: 0,
            category: 0,
            category_cooldown_ms: 0,
        };
        // Slot 1 names no row; slots 0 and 3 do.
        t.spells = vec![slot(0, 10, 3), slot(1, 11, 0), slot(3, 12, -1)];
        let mut spells = crate::ui_action::Spells::empty_for_tests();
        spells.catalog = benilla_formats::SpellCatalog::from_displays(HashMap::from([
            (
                10,
                SpellDisplay {
                    name: "Fireball".into(),
                    ..Default::default()
                },
            ),
            (
                12,
                SpellDisplay {
                    name: "Frost Nova".into(),
                    ..Default::default()
                },
            ),
        ]));
        let view = super::template_view(
            &t,
            Some(&spells),
            None,
            &|_| 0,
            Default::default(),
            None,
            &|| 0,
            &|_| None,
        );
        assert_eq!(view.item_level, 33);
        assert_eq!(
            view.spell_slots,
            [("Fireball".to_string(), 3), ("Frost Nova".to_string(), -1)]
        );
    }

    /// A zone-bound template names its area through `AreaTable.dbc` and its map through
    /// `Map.dbc` (`0x52bac9`..`0x52bb3e`); an id of 0 names nothing, though map 0 has a row.
    #[test]
    fn a_zone_bound_template_names_its_area_and_map() {
        let areas = benilla_formats::QuestHeaderNames::from_zones([(3428, "Ahn'Qiraj")]);
        let maps = benilla_formats::MapCatalog::from_rows([])
            .with_names([(0, "Eastern Kingdoms"), (531, "Ahn'Qiraj Temple")]);
        let tables = super::ViewTables {
            areas: Some(&areas),
            maps: Some(&maps),
            ..Default::default()
        };
        let view = |t: &benilla_protocol::ItemInfo| {
            let v = super::template_view(t, None, None, &|_| 0, tables, None, &|| 0, &|_| None);
            (v.area_name, v.map_name)
        };
        let mut t = crate::items::test_template("Scarab Coffer Key");
        t.area = 3428;
        t.map = 531;
        assert_eq!(
            view(&t),
            (
                Some("Ahn'Qiraj".to_string()),
                Some("Ahn'Qiraj Temple".to_string())
            )
        );
        t.area = 0;
        t.map = 0;
        assert_eq!(view(&t), (None, None));
    }

    /// Block 0's spell teaches when its Effect[0] is 36: the taught spell is EffectTriggerSpell[0]
    /// with its later ranks, a pet teach when an implicit target of effect 0 is 5
    /// (`0x52d4fd`..`0x52d54d`). Another block, or another effect, teaches nothing.
    #[test]
    fn block_zeros_learn_spell_names_the_taught_spell_and_its_ranks() {
        use benilla_formats::{SlaInfo, SpellDisplay};
        use benilla_ui::script::TaughtSpell;
        use std::collections::HashMap;

        let learn = |taught, target_b| SpellDisplay {
            effects: [36, 0, 0],
            effect_trigger_spell: [taught, 0, 0],
            effect_implicit_target_b: [target_b, 0, 0],
            ..Default::default()
        };
        let mut spells = crate::ui_action::Spells::empty_for_tests();
        spells.catalog = benilla_formats::SpellCatalog::from_displays(HashMap::from([
            (9000, learn(2335, 0)),
            (9001, learn(6000, 5)),
            (
                9002,
                SpellDisplay {
                    effects: [24, 36, 0],
                    effect_trigger_spell: [0, 2335, 0],
                    ..Default::default()
                },
            ),
        ]));
        let sla = |forward_spell_id| SlaInfo {
            skill_id: 171,
            req_skill_value: 0,
            forward_spell_id,
            trivial_low: 0,
            trivial_high: 0,
        };
        let lines =
            benilla_formats::SkillLineCatalog::from_abilities([(2335, sla(2336)), (2336, sla(0))]);
        let mut t = crate::items::test_template("Recipe");
        let teaches =
            |t: &benilla_protocol::ItemInfo| super::taught_spell(t, Some(&spells), Some(&lines));
        t.spells = vec![slot(9000, -1)];
        assert_eq!(
            teaches(&t),
            Some(TaughtSpell {
                ranks: vec![2335, 2336],
                pet: false
            })
        );
        t.spells = vec![slot(9001, -1)];
        assert_eq!(
            teaches(&t),
            Some(TaughtSpell {
                ranks: vec![6000],
                pet: true
            })
        );
        t.spells = vec![slot(9002, -1)];
        assert_eq!(teaches(&t), None, "effect 0 is no teach");
        t.spells = vec![ItemSpellEntry {
            index: 1,
            ..slot(9000, -1)
        }];
        assert_eq!(teaches(&t), None, "block 1 is not read");
    }

    /// A block teaching a spell that creates an item is a recipe: the product is the taught
    /// spell's EffectItemType[0] and the reagents its ids above 0 with their counts
    /// (`0x52daf5`..`0x52dbf8`); a teach of anything else, or no teach, is none.
    #[test]
    fn a_block_teaching_a_create_item_spell_is_a_recipe() {
        use benilla_formats::SpellDisplay;
        use std::collections::HashMap;

        let mut reagents = [(0, 0); 8];
        reagents[0] = (2452, 1);
        reagents[1] = (2450, 2);
        reagents[3] = (3371, 1);
        let mut spells = crate::ui_action::Spells::empty_for_tests();
        spells.catalog = benilla_formats::SpellCatalog::from_displays(HashMap::from([
            (
                2364,
                SpellDisplay {
                    effects: [36, 0, 0],
                    effect_trigger_spell: [2335, 0, 0],
                    ..Default::default()
                },
            ),
            (
                2335,
                SpellDisplay {
                    effects: [24, 0, 0],
                    effect_item_type: [2459, 0, 0],
                    reagents,
                    ..Default::default()
                },
            ),
            (
                2366,
                SpellDisplay {
                    effects: [36, 0, 0],
                    effect_trigger_spell: [2367, 0, 0],
                    ..Default::default()
                },
            ),
            (
                2367,
                SpellDisplay {
                    effects: [6, 0, 0],
                    ..Default::default()
                },
            ),
        ]));
        assert_eq!(
            super::recipe_of(Some(&spells), 2364),
            Some(benilla_ui::script::ItemRecipe {
                product: 2459,
                reagents: vec![(2452, 1), (2450, 2), (3371, 1)],
            })
        );
        assert_eq!(
            super::recipe_of(Some(&spells), 2366),
            None,
            "teaches an aura"
        );
        assert_eq!(super::recipe_of(Some(&spells), 2335), None, "no teach");
    }

    /// The requirement line's faction with no `Faction.dbc` row reads the literal `UNKNOWN`
    /// (`0x838044`), and a female player's label is the `_FEMALE` twin (`0x52d80f`, `0x612bf0`).
    #[test]
    fn a_reputation_line_genders_its_label_and_names_an_unknown_faction() {
        let strings = [
            ("ITEM_REQ_REPUTATION", "Requires %s - %s"),
            ("FACTION_STANDING_LABEL6", "Honored"),
            ("FACTION_STANDING_LABEL6_FEMALE", "Honored (f)"),
        ];
        let get = |k: &str| {
            strings
                .iter()
                .find(|(key, _)| *key == k)
                .map(|(_, v)| v.to_string())
        };
        let mut t = crate::items::test_template("Argent Dawn Tabard");
        t.required_rep_faction = 529;
        t.required_rep_rank = 5;
        let line = |gender: u8| {
            super::template_view(
                &t,
                None,
                None,
                &|_| 0,
                Default::default(),
                None,
                &move || gender,
                &get,
            )
            .required_rep_line
        };
        assert_eq!(line(0).as_deref(), Some("Requires UNKNOWN - Honored"));
        assert_eq!(line(1).as_deref(), Some("Requires UNKNOWN - Honored (f)"));
    }

    /// The build's lookups pass a callback (`0x52e204`, `0x52b93b`): a run asks a name still in
    /// flight no second time, and asks a refused one again at once.
    #[test]
    fn each_build_asks_its_missed_name_and_charter_again_once_answered() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let commands = crate::net::NetCommands(tx);
        let mut names = crate::names::NameCache::default();
        let petitions = crate::ui_petition::PetitionState::default();
        let run = |names: &crate::names::NameCache| {
            ask_build_lookups((vec![0x5], vec![0x99]), |_| 7, names, &petitions, &commands);
            crate::query_cache::queries(&rx).len()
        };
        assert_eq!(
            run(&names),
            2,
            "the first build asks the name and the record"
        );
        for _ in 0..600 {
            assert_eq!(run(&names), 0, "in flight: no build asks again");
        }
        names.insert_player(0x5, String::new(), None);
        assert_eq!(run(&names), 1, "a refused name is asked at the next build");
    }

    /// A bag slot carries the template's quality and InventoryType, which `GetContainerItemInfo`
    /// gates its fourth value on (`0x4f9794`); an unanswered template leaves them empty.
    #[test]
    fn a_resolved_slot_carries_the_templates_quality_and_inventory_type() {
        use crate::items::{test_template, Items, RollCatalogs};
        use crate::names::NameCache;
        use crate::ui_items::TestObjects;
        use benilla_protocol::ObjectFields;

        const ENTRY: u16 = 3; // OBJECT_FIELD_ENTRY
        let mut objs = TestObjects::new();
        // The potion has 3 charges left in ITEM_FIELD_SPELL_CHARGES[0] (16), its neighbours set.
        const CHARGES_0: u16 = 16;
        for (guid, entry, charges) in [(0xA1_u64, 1234_u32, 0), (0xA2, 1235, -3), (0xA3, 1236, 0)] {
            objs.spawn(
                guid,
                ObjectFields::from_pairs(&[
                    (ENTRY, entry),
                    (14, 1),
                    (CHARGES_0, charges as u32),
                    (17, 7),
                ]),
            );
        }
        let mut items = Items::default();
        let mut sword = test_template("Sword");
        sword.quality = 4;
        sword.inventory_type = 13;
        sword.class = 2;
        let sword_class = sword.class;
        items.insert_template(1234, Some(sword));
        items.insert_template(1235, Some(test_template("Healing Potion")));

        let names = NameCache::default();
        let cooldowns = crate::spell::Cooldowns::default();
        let petitions = crate::ui_petition::PetitionState::default();
        let mut slot = |guid: u64| {
            resolve_slot(
                guid,
                &objs.get(),
                &items,
                None,
                RollCatalogs::NONE,
                &cooldowns,
                None,
                &names,
                &petitions,
                std::time::Instant::now(),
                0.0,
                false,
            )
        };
        // A guid whose item object has not streamed reads as no item at all (`0x6228a0` finds
        // none), not as an occupied blank.
        assert!(slot(0xA9).is_none());
        let sword = slot(0xA1).expect("an occupied slot");
        assert_eq!((sword.quality, sword.inventory_type), (Some(4), 13));
        let potion = slot(0xA2).expect("an occupied slot");
        assert_eq!((potion.quality, potion.inventory_type), (Some(1), 0));
        // The merchant price's charge scale reads the object's own (`flds+0x28`, `0x52e43d`).
        assert_eq!((potion.spell_charges[0], sword.spell_charges[0]), (-3, 0));
        let pending = slot(0xA3).expect("an occupied slot");
        assert_eq!((pending.quality, pending.inventory_type), (None, 0));
        // The guid and the class: the lock reads the one from the first push, and the
        // auto-equip's unlock reads the other (`0x5e1704`).
        assert_eq!((sword.guid, sword.item_class), (0xA1, Some(sword_class)));
        assert_eq!((pending.guid, pending.item_class), (0xA3, None));
    }

    /// A charter's slot carries the signature count beside its petition record: the object's
    /// slot-0 enchantment duration (`flds+0x44`, `0x52ba51`).
    #[test]
    fn a_charters_slot_carries_its_signature_count() {
        use crate::items::{test_template, Items, RollCatalogs};
        use crate::names::NameCache;
        use crate::ui_items::TestObjects;
        use benilla_protocol::ObjectFields;

        let mut objs = TestObjects::new();
        // OBJECT_FIELD_ENTRY 3; the petition id at 22, the count at 23, the charges at 24.
        objs.spawn(
            0xC1,
            ObjectFields::from_pairs(&[(3, 5863), (22, 0x51), (23, 4), (24, 9)]),
        );
        let mut items = Items::default();
        let mut charter = test_template("Guild Charter");
        charter.flags = benilla_protocol::messages::ITEM_FLAG_CHARTER;
        items.insert_template(5863, Some(charter));
        let mut petitions = crate::ui_petition::PetitionState::default();
        crate::ui_petition::net::query_response(
            &mut petitions,
            benilla_protocol::messages::PetitionQueryResponse {
                petition_id: 0x51,
                name: "Pact".into(),
                ..Default::default()
            },
        );
        let slot = resolve_slot(
            0xC1,
            &objs.get(),
            &items,
            None,
            RollCatalogs::NONE,
            &crate::spell::Cooldowns::default(),
            None,
            &NameCache::default(),
            &petitions,
            std::time::Instant::now(),
            0.0,
            false,
        )
        .expect("an occupied slot");
        let petition = slot.petition.expect("the record is cached");
        assert_eq!((petition.title.as_str(), petition.signatures), ("Pact", 4));
    }

    /// A bag slot holding a container says how many of its seats are filled, which
    /// `ClickTradeButton` refuses a bag on (`0x4bff50`); any other item says nothing.
    #[test]
    fn a_resolved_bag_counts_its_filled_seats() {
        use crate::items::{test_template, Items, RollCatalogs};
        use crate::names::NameCache;
        use crate::ui_items::TestObjects;
        use benilla_protocol::field::{FIELD_CONTAINER_SLOT_1, FIELD_OBJECT_ENTRY};
        use benilla_protocol::ObjectFields;

        // `OBJECT_FIELD_TYPE` item or container, `CONTAINER_FIELD_NUM_SLOTS`.
        const TYPE: u16 = 2;
        const NUM_SLOTS: u16 = 48;
        let mut objs = TestObjects::new();
        let seat = |j: u16, g: u64| {
            [
                (FIELD_CONTAINER_SLOT_1 + 2 * j, g as u32),
                (FIELD_CONTAINER_SLOT_1 + 2 * j + 1, 0),
            ]
        };
        let bag = |contents: &[(u16, u64)]| {
            let mut pairs = vec![(TYPE, 0x7), (FIELD_OBJECT_ENTRY, 4496), (NUM_SLOTS, 6)];
            pairs.extend(contents.iter().flat_map(|&(j, g)| seat(j, g)));
            ObjectFields::from_pairs(&pairs)
        };
        objs.spawn(0xB1, bag(&[(0, 0xC1), (4, 0xC2)]));
        objs.spawn(0xB2, bag(&[]));
        objs.spawn(
            0xB3,
            ObjectFields::from_pairs(&[(TYPE, 0x3), (FIELD_OBJECT_ENTRY, 4496)]),
        );
        let mut items = Items::default();
        items.insert_template(4496, Some(test_template("Small Brown Pouch")));
        let names = NameCache::default();
        let cooldowns = crate::spell::Cooldowns::default();
        let petitions = crate::ui_petition::PetitionState::default();
        let mut seats = |guid: u64| {
            resolve_slot(
                guid,
                &objs.get(),
                &items,
                None,
                RollCatalogs::NONE,
                &cooldowns,
                None,
                &names,
                &petitions,
                std::time::Instant::now(),
                0.0,
                false,
            )
            .expect("an occupied slot")
            .occupied_seats
        };
        assert_eq!(seats(0xB1), Some(2));
        assert_eq!(seats(0xB2), Some(0));
        assert_eq!(seats(0xB3), None, "not a container");
    }

    /// A worn item answers the item cooldown query by its entry (`0x6e2ed0`): its answer is pushed
    /// under the live-API slot id, one past the descriptor slot. An on-use item with none running
    /// answers cold and enabled (`0x6e1406`); one with no use spell is absent, the query leaving
    /// its zeros (`0x6e2f34`).
    #[test]
    fn a_worn_item_on_cooldown_is_keyed_by_its_live_slot_id() {
        use super::worn_cooldowns;
        use crate::items::{test_template, Items};
        use crate::net::ObjectStore;
        use crate::ui_items::TestObjects;
        use benilla_protocol::field::{FIELD_OBJECT_ENTRY, FIELD_PLAYER_INV_SLOT_HEAD};
        use benilla_protocol::messages::ItemUseSpell;
        use benilla_protocol::ObjectFields;

        let mut objs = TestObjects::new();
        for (guid, entry) in [(0xA1_u64, 2000_u32), (0xA2, 2001), (0xA3, 2002)] {
            objs.spawn(
                guid,
                ObjectFields::from_pairs(&[(FIELD_OBJECT_ENTRY, entry)]),
            );
        }
        let use_spell = |spell_id| ItemUseSpell {
            spell_id,
            cooldown_ms: 120_000,
            category: 0,
            category_cooldown_ms: 0,
        };
        let mut items = Items::default();
        for (entry, spell) in [
            (2000, Some(use_spell(9000))),
            (2001, Some(use_spell(9001))),
            (2002, None),
        ] {
            let mut t = test_template("Trinket");
            t.use_spell = spell;
            items.insert_template(entry, Some(t));
        }
        let now = std::time::Instant::now();
        let mut cooldowns = crate::spell::Cooldowns::default();
        // Only 2000 was used; 2001 has a use spell and no cooldown, 2002 has no use spell.
        cooldowns.start_item(2000, &use_spell(9000), None, now);
        // Descriptor slots 12 (the trinket), 13 and 3 hold the three items.
        let slot = |s: u16, guid: u64| {
            [
                (FIELD_PLAYER_INV_SLOT_HEAD + 2 * s, guid as u32),
                (FIELD_PLAYER_INV_SLOT_HEAD + 2 * s + 1, 0),
            ]
        };
        let pairs: Vec<(u16, u32)> = [slot(12, 0xA1), slot(13, 0xA2), slot(3, 0xA3)]
            .into_iter()
            .flatten()
            .collect();
        let store = ObjectStore(ObjectFields::from_pairs(&pairs));

        let worn = worn_cooldowns(&store, &objs.get(), &items, &cooldowns, None, now, 50.0);
        assert_eq!(
            worn,
            HashMap::from([(13, (50_000, 120_000, true)), (14, (0, 0, true))]),
            "descriptor slot 12 is live id 13; 2001 is cold, enabled; 2002 has no use spell"
        );
    }

    /// A charged template whose copy has no charge left skips the cooldown query, in a bag and on
    /// the doll alike (`0x5347e3`-`0x5347f4`, `0x53318a`-`0x53319b`, `0x4f9a48`, `0x4c8b55`):
    /// the test is `template+0x144 != 0` (-1 included) against `ITEM_FIELD_SPELL_CHARGES[0] == 0`.
    #[test]
    fn a_charged_item_with_no_charge_left_has_no_cooldown() {
        use super::worn_cooldowns;
        use crate::items::{test_template, Items, RollCatalogs};
        use crate::names::NameCache;
        use crate::net::ObjectStore;
        use crate::ui_items::TestObjects;
        use benilla_protocol::field::{
            FIELD_ITEM_SPELL_CHARGES, FIELD_OBJECT_ENTRY, FIELD_PLAYER_INV_SLOT_HEAD,
        };
        use benilla_protocol::messages::ItemUseSpell;
        use benilla_protocol::ObjectFields;

        let use_spell = |spell_id| ItemUseSpell {
            spell_id,
            cooldown_ms: 120_000,
            category: 0,
            category_cooldown_ms: 0,
        };
        // (guid, entry, template charges, the copy's charges): five cases of the one test.
        let cases: [(u64, u32, i32, i32); 5] = [
            (0xD1, 3000, 5, 0),   // charged, spent: no cooldown
            (0xD2, 3001, 5, 3),   // charged, 3 left: the cooldown
            (0xD3, 3002, -1, 0),  // unlimited template, copy at 0: no cooldown (`!= 0`, not finite)
            (0xD4, 3003, 0, 0),   // uncharged template: the cooldown
            (0xD5, 3004, -1, -1), // unlimited and unspent: the cooldown
        ];
        let mut objs = TestObjects::new();
        let mut items = Items::default();
        let now = std::time::Instant::now();
        let mut cooldowns = crate::spell::Cooldowns::default();
        for &(guid, entry, template_charges, copy_charges) in &cases {
            objs.spawn(
                guid,
                ObjectFields::from_pairs(&[
                    (FIELD_OBJECT_ENTRY, entry),
                    (FIELD_ITEM_SPELL_CHARGES, copy_charges as u32),
                ]),
            );
            let mut t = test_template("Charged");
            t.use_spell = Some(use_spell(entry + 6000));
            t.spell_charges_0 = template_charges;
            items.insert_template(entry, Some(t));
            cooldowns.start_item(entry, &use_spell(entry + 6000), None, now);
        }
        let running = Some((50_000, 120_000, true));
        let expected = [None, running, None, running, running];

        let names = NameCache::default();
        let petitions = crate::ui_petition::PetitionState::default();
        for (&(guid, ..), want) in cases.iter().zip(expected) {
            let slot = resolve_slot(
                guid,
                &objs.get(),
                &items,
                None,
                RollCatalogs::NONE,
                &cooldowns,
                None,
                &names,
                &petitions,
                now,
                50.0,
                false,
            )
            .expect("an occupied slot");
            assert_eq!(slot.cooldown, want, "bag slot of {guid:#x}");
        }

        // The same five on the doll, descriptor slots 12..=16.
        let pairs: Vec<(u16, u32)> = cases
            .iter()
            .enumerate()
            .flat_map(|(i, &(guid, ..))| {
                let base = FIELD_PLAYER_INV_SLOT_HEAD + 2 * (12 + i as u16);
                [(base, guid as u32), (base + 1, 0)]
            })
            .collect();
        let store = ObjectStore(ObjectFields::from_pairs(&pairs));
        let worn = worn_cooldowns(&store, &objs.get(), &items, &cooldowns, None, now, 50.0);
        let want: HashMap<usize, (i64, u32, bool)> = expected
            .iter()
            .enumerate()
            .filter_map(|(i, w)| w.map(|w| (13 + i, w)))
            .collect();
        assert_eq!(worn, want);
    }

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
        apply_container_source(&mut s, &mut memory, Some(vault(1, 5)), guids, Vec::new());
        assert_eq!(seen(&mut s), vec!["PLAYERBANKSLOTS_CHANGED nil"]);

        // The same item restacks: the item watcher's path, with the unit token.
        apply_container_source(&mut s, &mut memory, Some(vault(1, 9)), guids, Vec::new());
        assert_eq!(seen(&mut s), vec!["PLAYERBANKSLOTS_CHANGED player"]);

        apply_container_source(&mut s, &mut memory, Some(vault(1, 9)), guids, Vec::new());
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
        apply_container_source(&mut s, &mut memory, Some(two()), guids, Vec::new());
        assert_eq!(seen(&mut s), vec!["PLAYERBANKSLOTS_CHANGED nil"]);
        guids.vault.swap(0, 1);
        let pushed = apply_container_source(&mut s, &mut memory, Some(two()), guids, Vec::new());
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
        apply_container_source(&mut s, &mut memory, Some(pouch(6)), guids, Vec::new());
        assert_eq!(seen(&mut s), vec!["BAG_UPDATE 1"]);

        apply_container_source(&mut s, &mut memory, Some(pouch(6)), guids, Vec::new());
        assert!(
            seen(&mut s).is_empty(),
            "an unchanged container says nothing"
        );

        // Swapped for another bag: closed, then updated.
        guids.bags[0] = 0xBBBB;
        apply_container_source(&mut s, &mut memory, Some(pouch(10)), guids, Vec::new());
        assert_eq!(seen(&mut s), vec!["BAG_CLOSED 1", "BAG_UPDATE 1"]);

        // Unequipped: closed, and no `BAG_UPDATE`.
        guids.bags[0] = 0;
        apply_container_source(&mut s, &mut memory, Some(HashMap::new()), guids, Vec::new());
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
        apply_container_source(&mut s, &mut memory, Some(two()), guids, Vec::new());
        let _ = seen(&mut s);
        guids.bags.swap(4, 5);
        let pushed = apply_container_source(&mut s, &mut memory, Some(two()), guids, Vec::new());
        assert_eq!(seen(&mut s), vec!["BAG_CLOSED 5", "BAG_CLOSED 6"]);
        assert!(
            pushed,
            "a BAG_CLOSED with no container change still went into the VM — the gate audit reads \
             this return"
        );
    }
    use crate::ui_action::real_spells;
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
            name: None,
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
        );
        assert_eq!(s.eval::<i64>("return GetContainerNumSlots(0)").unwrap(), 16);
        assert_eq!(s.eval::<i64>("return BAG_EVENTS").unwrap(), 1);

        // The logout despawn frame: the VM keeps its bags and nothing fires.
        apply_container_source(&mut s, &mut memory, None, NO_BAGS, Vec::new());
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
        );
        assert_eq!(s.eval::<i64>("return GetContainerNumSlots(0)").unwrap(), 0);
        assert_eq!(s.eval::<i64>("return BAG_EVENTS").unwrap(), 2);
    }

    /// The feed fires only the bag's `BAG_UPDATE` for a restack: the stack-count write's
    /// `ITEM_LOCK_CHANGED` is the lock watcher's, fired ahead of this push
    /// ([`crate::ui_items::locks::apply_item_releases`]).
    #[test]
    fn a_restack_is_a_bag_update_alone_to_the_feed() {
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
        let bag = |count: u32| {
            HashMap::from([(
                0,
                ContainerState {
                    name: None,
                    num_slots: 16,
                    slots: HashMap::from([(
                        1,
                        ContainerSlot {
                            item_id: 2512, // Rough Arrow
                            count,
                            ..Default::default()
                        },
                    )]),
                },
            )])
        };
        let mut memory = FeedMemory::default();
        for count in [200, 199] {
            s.run("ORDER = {}").unwrap();
            apply_container_source(&mut s, &mut memory, Some(bag(count)), NO_BAGS, Vec::new());
            assert_eq!(
                s.eval::<String>("return table.concat(ORDER, ' ')").unwrap(),
                "BAG_UPDATE:0,nil",
                "{count}"
            );
        }
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

    #[test]
    fn modifier_refresh_tracks_only_item_and_set_descriptions_with_tokens() {
        let displays = std::collections::HashMap::from([
            (
                1,
                benilla_formats::SpellDisplay {
                    description: Some("Restores 5 health.".into()),
                    ..Default::default()
                },
            ),
            (
                2,
                benilla_formats::SpellDisplay {
                    description: Some("Restores $s1 health.".into()),
                    ..Default::default()
                },
            ),
        ]);
        let spells = crate::ui_action::Spells {
            catalog: benilla_formats::SpellCatalog::from_displays(displays),
            forms: Default::default(),
            ranges: Default::default(),
            cast_times: Default::default(),
            durations: Default::default(),
            radii: Default::default(),
        };
        let mut item = crate::items::test_template("Token item");
        item.spells = vec![slot(1, 0)];
        assert!(!super::item_has_mod_tokens(&item, Some(&spells)));
        item.spells.push(slot(2, 0));
        assert!(super::item_has_mod_tokens(&item, Some(&spells)));
        assert!(!super::bonuses_have_mod_tokens(&[(2, 1)], Some(&spells)));
        assert!(super::bonuses_have_mod_tokens(
            &[(2, 1), (4, 2)],
            Some(&spells)
        ));
    }

    /// On the real Spell.dbc, an undescribed spell such as a key's `Opening` builds no trigger
    /// line (`0x52da29`-`0x52da31`), and a described one does.
    #[test]
    fn an_undescribed_spell_prints_no_trigger_line_on_real_data() {
        let Some(spells) = real_spells() else { return };

        // No string table: Fireball's description reaches no keyed token.
        let no_strings = |_: &str| None;

        // The lock chain's Opening and Closing spells, none with a description.
        for id in [3365u32, 3366, 6246, 6247, 6477, 21651] {
            let d = spells.catalog.get(id).expect("a real Spell.dbc row");
            assert!(
                !d.name.is_empty(),
                "spell {id} has a name — which is exactly what must NOT leak into the tooltip"
            );
            assert_eq!(
                super::spell_desc_text(Some(&spells), id, &|_| 0, None, &|| 0, None, &no_strings),
                None,
                "spell {id} ({:?}) has no description, so the reference prints no trigger line",
                d.name
            );
        }

        // The control: Fireball (133), described, still yields its line.
        let fireball =
            super::spell_desc_text(Some(&spells), 133, &|_| 0, None, &|| 0, None, &no_strings)
                .expect("a described spell still yields its line");
        assert!(
            fireball.contains("damage"),
            "expected the substituted Fireball description, got {fireball:?}"
        );
    }

    /// The trigger text's `$g` branch reads the gender handed in (`0x508214`): Conjure Food 587's
    /// "providing the mage and `$ghis:her;` allies" takes "her" on 1, "his" on 0.
    #[test]
    fn a_trigger_texts_gender_branch_follows_the_player() {
        let Some(spells) = real_spells() else { return };
        let no_strings = |_: &str| None;
        let line = |gender: u8| {
            super::spell_desc_text(
                Some(&spells),
                587,
                &|_| 0,
                None,
                &move || gender,
                None,
                &no_strings,
            )
            .expect("a described spell")
        };
        assert!(line(1).contains("the mage and her allies"), "{}", line(1));
        assert!(line(0).contains("the mage and his allies"), "{}", line(0));
    }

    /// The item feeds re-substitute a held `$`-text when what the tokens read moves: the gender
    /// byte `$g`/`$G` branch on (`0x508214`), and no other byte of `UNIT_FIELD_BYTES_0`.
    #[test]
    fn the_token_reads_move_with_the_gender_byte_alone() {
        let store = |bytes: u32| {
            crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(36, bytes)]))
        };
        let male = super::token_reads(Some(&store(4 | 1 << 8)));
        assert_ne!(super::token_reads(Some(&store(4 | 1 << 8 | 1 << 16))), male);
        assert_eq!(super::token_reads(Some(&store(1 | 2 << 8 | 1 << 24))), male);
    }
}

#[cfg(test)]
#[path = "feed_cooldown_tests.rs"]
mod cooldown_tests;

#[cfg(test)]
mod ask_tests {
    use bevy::prelude::*;

    use crate::items::Items;
    use crate::net::NetCommands;

    /// A `/reload` keeps the reference's item cache, so a new VM is seated with every cached
    /// record its first frame, ahead of its world entry, and its first read hits.
    #[test]
    fn a_new_vm_is_seated_with_every_cached_template() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<Items>()
            .init_resource::<crate::spell::SpellModifiers>()
            .add_systems(Update, super::feed_item_stats);
        app.world_mut()
            .resource_mut::<Items>()
            .insert_template(25, Some(crate::items::test_template("Linen Cloth")));
        let name = |app: &mut App| {
            app.world_mut()
                .non_send_resource_mut::<benilla_ui::script::UiScript>()
                .eval::<Option<String>>("return (GetItemInfo(25))")
                .unwrap()
        };
        app.insert_non_send_resource(benilla_ui::script::UiScript::new().unwrap());
        app.update();
        assert_eq!(name(&mut app).as_deref(), Some("Linen Cloth"));
        app.insert_non_send_resource(benilla_ui::script::UiScript::new().unwrap());
        app.update();
        assert_eq!(
            name(&mut app).as_deref(),
            Some("Linen Cloth"),
            "the new VM's"
        );
        assert!(crate::query_cache::queries(&rx).is_empty());
    }

    /// A miss the VM asks for is the getter's lookup with a callback: it queries, a frame between
    /// asks queries nothing, and the next ask queries a refused id again. A miss `GetItemInfo`
    /// reads never queries (`0x48e172`).
    #[test]
    fn a_refused_template_is_asked_by_the_next_getter_and_never_by_get_item_info() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<Items>()
            .init_resource::<crate::spell::SpellModifiers>()
            .add_systems(Update, super::feed_item_stats);
        app.insert_non_send_resource(benilla_ui::script::UiScript::new().unwrap());
        let run = |app: &mut App, lua: &str| {
            app.world_mut()
                .non_send_resource_mut::<benilla_ui::script::UiScript>()
                .run(lua)
                .unwrap();
            app.update();
        };
        run(&mut app, "GetItemInfo(25)");
        assert!(
            crate::query_cache::queries(&rx).is_empty(),
            "a read asks nothing"
        );
        let ask = |app: &mut App| {
            app.world_mut()
                .non_send_resource_mut::<benilla_ui::script::UiScript>()
                .ask_item_templates([25]);
            app.update();
        };
        ask(&mut app);
        assert_eq!(crate::query_cache::queries(&rx).len(), 1);
        app.world_mut()
            .resource_mut::<Items>()
            .insert_template(25, None);
        for _ in 0..600 {
            run(&mut app, "GetItemInfo(25)");
        }
        assert!(
            crate::query_cache::queries(&rx).is_empty(),
            "no frame or read asks a refusal"
        );
        ask(&mut app);
        assert_eq!(
            crate::query_cache::queries(&rx).len(),
            1,
            "the next getter asks again"
        );
    }

    /// A chat link's bare enchant id names its line through the table pushed at load: every named
    /// `SpellItemEnchantment` row, one whose `Flags & 0x2` silences the combat log included, as
    /// the builder's loop tests no flag before it prints the name (`0x52ca23`..`0x52ca8b`).
    #[test]
    fn a_links_enchant_names_its_line_from_the_table_pushed_at_load() {
        let mut app = App::new();
        app.insert_resource(crate::items::Enchants(
            benilla_formats::EnchantCatalog::from_rows(
                std::collections::HashMap::new(),
                std::collections::HashMap::from([(29, "Rockbiter 3".to_string())]),
                std::collections::HashMap::from([(29, 0x2)]),
            ),
        ))
        .add_systems(Update, super::feed_random_properties);
        let mut script = benilla_ui::script::UiScript::new().unwrap();
        crate::ui_script::test_ui::load_tooltip_template(&script);
        script.set_item_template(
            9300,
            benilla_ui::script::ItemTemplateView {
                name: "Runed Blade".into(),
                quality: 2,
                ..Default::default()
            },
        );
        app.insert_non_send_resource(script);
        app.update();
        let line: String = app
            .world_mut()
            .non_send_resource_mut::<benilla_ui::script::UiScript>()
            .eval(
                r#"
                local tt = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
                tt:SetOwner(CreateFrame("Frame"), "ANCHOR_NONE")
                tt:SetHyperlink("item:9300:29:0:0")
                return TTTextLeft2:GetText()
                "#,
            )
            .unwrap();
        assert_eq!(line, "Rockbiter 3");
    }

    /// The requirement state carries `PLAYER_FIELD_PVP_MEDALS` and the whole known-spell book: a
    /// held medal whites its line (`0x52d70a`), and a recipe whose craft the book holds, which the
    /// spellbook never lists, reads already known (`0x52d54f`).
    #[test]
    fn the_requirement_feed_pushes_the_medals_and_the_known_spells() {
        let mut app = App::new();
        app.init_resource::<crate::net::Proficiencies>()
            .init_resource::<crate::net::Reputations>()
            .init_resource::<crate::ui_action::PlayerActions>()
            .init_resource::<crate::ui_action::LearnedAbilities>()
            .add_systems(Update, super::feed_player_req);
        app.world_mut()
            .resource_mut::<crate::ui_action::PlayerActions>()
            .spells
            .insert(2335);
        // UNIT_FIELD_LEVEL 34 and PLAYER_FIELD_PVP_MEDALS 1225, medal 1 held.
        app.world_mut().spawn((
            crate::net::SelfPlayer,
            crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (34, 60),
                (1225, 0x1),
            ])),
        ));
        let mut script = benilla_ui::script::UiScript::new().unwrap();
        crate::ui_script::test_ui::load_tooltip_template(&script);
        script.set_item_template(
            9504,
            benilla_ui::script::ItemTemplateView {
                name: "Recipe: Medal".into(),
                quality: 1,
                required_city_rank: 1,
                teaches: Some(benilla_ui::script::TaughtSpell {
                    ranks: vec![2335],
                    pet: false,
                }),
                ..Default::default()
            },
        );
        app.insert_non_send_resource(script);
        app.update();
        let (known, medal, r, g, b): (String, String, f32, f32, f32) = app
            .world_mut()
            .non_send_resource_mut::<benilla_ui::script::UiScript>()
            .eval(
                r#"
                ITEM_SPELL_KNOWN = "Already known"
                ITEM_REQ_SKILL = "Requires %s"
                PVP_MEDAL1 = "Medal"
                local tt = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
                tt:SetOwner(CreateFrame("Frame"), "ANCHOR_NONE")
                tt:SetHyperlink("item:9504:0:0:0")
                local r, g, b = TTTextLeft3:GetTextColor()
                return TTTextLeft2:GetText(), TTTextLeft3:GetText(), r, g, b
                "#,
            )
            .unwrap();
        assert_eq!(
            (known.as_str(), medal.as_str()),
            ("Already known", "Requires Medal")
        );
        // White, not the unmet red's `32/255` green and blue.
        assert!(
            [r, g, b].iter().all(|c| (c - 1.0).abs() < 1e-4),
            "the medal is held: {:?}",
            (r, g, b)
        );
    }
}
