//! The per-frame feeds into the VM: the item-tooltip views every window reads; and the one bag
//! slot's derivation the container getters read at the call ([`resolve_slot`], [`super::bags`]).

use bevy::prelude::*;

use benilla_protocol::ItemInfo;
use benilla_ui::script::{ContainerSlot, ItemRead, UiScript};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::{NetCommands, ObjectStore, Objects, SelfPlayer};
use crate::query_cache::Ask;

use super::find_equip_slot;

/// What the `$`-tokens read of the caster, the active player for an item's lines (the expander's
/// pet argument is 0, `0x52da24`): its level for a spell, the per-level terms' level
/// (`TokenContext::level`), and the cost `$c` and the per-second cost `$p` print.
#[derive(Clone, Copy)]
struct TokenCaster<'a> {
    level: &'a dyn Fn(&benilla_formats::SpellDisplay) -> u32,
    power_cost: &'a dyn Fn(&benilla_formats::SpellDisplay) -> i32,
    power_per_second: &'a dyn Fn(&benilla_formats::SpellDisplay) -> i32,
}

#[cfg(test)]
impl TokenCaster<'static> {
    /// No player: every level 0, no cost to read (`0x6e3233`), and the per-second cost at level 0.
    const NONE: Self = Self {
        level: &|_| 0,
        power_cost: &|_| -1,
        power_per_second: &|d| {
            crate::spell::usable::per_second_cost(d, crate::spell::SpellLevel::Known(0))
        },
    };
}

/// A spell's `$`-substituted description, for item trigger and set-bonus lines. An empty one
/// yields `None` and the caller drops the whole line, prefix included: the item builder
/// (`0x52d8a0`) expands it (`0x5075f0`), tests the first byte (`0x52da29`) and on zero skips the
/// trigger block (`0x52da31`), so an undescribed spell such as a key's `Opening` prints no line.
fn spell_desc_text(
    spells: Option<&crate::ui_action::Spells>,
    id: u32,
    caster: TokenCaster,
    home_area: Option<&str>,
    // The active player's `UNIT_FIELD_BYTES_0` byte 2, the `$g`/`$G` branch's input (`0x508214`).
    gender: &dyn Fn() -> u8,
    mods: Option<&crate::spell::SpellModifiers>,
    // The VM's strings for the keyed `$d`/`$s` tokens.
    global: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    let parts = spell_desc_parts(spells, id, caster, home_area, gender, mods)?;
    Some(benilla_formats::resolve_parts(
        &parts,
        global,
        &crate::ui_script::token_printf,
    ))
    .filter(|t| !t.is_empty())
}

/// [`spell_desc_text`]'s expansion with each template a token names left to be read where the
/// line shows; `None` for no description.
fn spell_desc_parts(
    spells: Option<&crate::ui_action::Spells>,
    id: u32,
    caster: TokenCaster,
    home_area: Option<&str>,
    gender: &dyn Fn() -> u8,
    mods: Option<&crate::spell::SpellModifiers>,
) -> Option<Vec<benilla_formats::TokenPart>> {
    let sp = spells?;
    let d = sp.catalog.get(id)?;
    let desc = d.description.as_deref().filter(|t| !t.is_empty())?;
    let ctx = benilla_formats::TokenContext {
        durations: &sp.durations,
        radii: &sp.radii,
        ranges: Some(&sp.ranges),
        level: caster.level,
        lookup: &|i| sp.catalog.get(i),
        mods: mods.map(|m| m as &dyn benilla_formats::SpellMods),
        power_cost: caster.power_cost,
        power_per_second: caster.power_per_second,
        unmodified_points: false,
        gender,
        home_area: &|| home_area,
        // Never read: the parts leave each template to the read where the line shows.
        global: &|_| None,
        printf: &crate::ui_script::token_printf,
    };
    Some(benilla_formats::substitute_parts(
        desc,
        d,
        &ctx,
        benilla_formats::ExpandAt::default(),
    ))
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
    caster: TokenCaster,
    tables: ViewTables,
    home_area: Option<&str>,
    // The active player's gender byte, the trigger text's `$g`/`$G` branch (`0x508214`) and the
    // standing label's (`0x612bf0`).
    gender: &dyn Fn() -> u8,
    // The VM's own `GlobalStrings.lua` and its `GetText`, for the reputation-requirement line
    // and the `$`-engine's keyed tokens.
    strings: benilla_ui::strings::Strings<'_>,
) -> benilla_ui::script::ItemTemplateView {
    let get = |key: &str| strings.global(key);
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
    let spell_text = |id: u32| spell_desc_text(spells, id, caster, home_area, gender, mods, &get);
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
        // `GetItemInfo`'s type pair, as it reads them as it runs.
        item_type: super::live::ClassNames {
            classes,
            sub_classes,
        }
        .item_type(t),
        item_sub_type: super::live::ClassNames {
            classes,
            sub_classes,
        }
        .item_sub_type(t),
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
                let standing = benilla_ui::strings::standing_label(
                    strings,
                    t.required_rep_rank,
                    gender() == 1,
                );
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
        area: t.area,
        map: t.map,
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

/// Set `set_id`'s block as the set block reads it at the call (`0x52d8a0..0x52e0f5`): the
/// `ItemSet.dbc` row, the members named off the item-name cache, only read (the build asks each
/// one still unnamed in its call, `0x52df4b`), the skill line and the bonuses' descriptions
/// expanded against the active player, `me`; `None` for no row.
pub(super) fn item_set_live(
    set_id: u32,
    sets: &super::ItemSets,
    item_names: &crate::item_names::ItemNames,
    spells: Option<&crate::ui_action::Spells>,
    spell_mods: Option<&crate::spell::SpellModifiers>,
    skill_lines: Option<&crate::ui_spellbook::SkillLines>,
    items: Option<&Items>,
    me: Option<&ObjectStore>,
) -> Option<benilla_ui::script::LiveItemSet> {
    let row = sets.0.set(set_id)?;
    let skill_catalog = skill_lines.map(|s| &s.catalog);
    let skills = crate::spell::PlayerSkills {
        lines: skill_catalog,
        spells: spells.map(|s| &s.catalog),
        items,
    };
    let level = |d: &benilla_formats::SpellDisplay| crate::spell::player_spell_level(me, skills, d);
    let power_cost = |d: &benilla_formats::SpellDisplay| match (me, spell_mods) {
        (Some(s), Some(mods)) => crate::spell::usable::power_cost(d, s, skills, mods),
        _ => -1,
    };
    let power_per_second = |d: &benilla_formats::SpellDisplay| {
        let level = crate::spell::SpellLevel::Player { me, skills };
        crate::spell::usable::per_second_cost(d, level)
    };
    let caster = TokenCaster {
        level: &level,
        power_cost: &power_cost,
        power_per_second: &power_per_second,
    };
    // The `$g`/`$G` branch reads our gender (`0x508214`).
    let gender = token_reads(me).1;
    Some(benilla_ui::script::LiveItemSet {
        view: benilla_ui::script::ItemSetView {
            name: row.name.clone(),
            members: set_members(&row.items, item_names, Ask::Read),
            bonuses: Vec::new(),
            required_skill: row.required_skill,
            required_skill_rank: row.required_skill_rank,
            required_skill_name: (row.required_skill != 0)
                .then(|| {
                    skill_catalog
                        .and_then(|sl| sl.line(row.required_skill))
                        .map(|l| l.name.clone())
                })
                .flatten(),
        },
        bonuses: row
            .bonuses
            .iter()
            .filter_map(|&(n, spell)| {
                let parts = spell_desc_parts(spells, spell, caster, None, &|| gender, spell_mods)?;
                Some((n, crate::ui_tooltip::tooltip_text(parts)))
            })
            .collect(),
    })
}

/// An item set's member names, each read as `ask` says from the item-name store, as the tooltip's
/// set block reads them (`0x52df5a` on `0xc0e2dc`), not from the template store.
pub(super) fn set_members(
    ids: &[u32],
    names: &crate::item_names::ItemNames,
    ask: Ask<'_>,
) -> Vec<(u32, Option<String>)> {
    ids.iter()
        .map(|&id| (id, names.name(id, ask).map(str::to_string)))
        .collect()
}

/// Pushes the whole random-suffix table once per VM, when both catalogs are in, and the enchant
/// names once the enchant catalog is: their consumers are click-driven, and a chat-link tooltip
/// has no hover loop to repaint a late answer. Both are load-once tables no packet writes.
pub(super) fn feed_random_properties(
    mut vm: crate::script_calls::VmWith<Option<Res<'static, crate::items::RandomProperties>>>,
    enchants: Option<Res<crate::items::Enchants>>,
    mut pushed: Local<crate::ui_script::VmMemo<bool>>,
    mut names_pushed: Local<crate::ui_script::VmMemo<bool>>,
) {
    vm.turn(|script, props| {
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
        let (Some(props), Some(enchants)) = (props.as_deref(), enchants) else {
            return;
        };
        let rows = crate::items::random_property_views(props, Some(&enchants));
        let with_lines = rows.values().filter(|v| !v.enchants.is_empty()).count();
        info!(
            "random-property table: {} rows pushed, {with_lines} with enchant lines",
            rows.len()
        );
        script.set_random_properties(rows);
        *pushed.get(script) = true;
    })
}

/// The name formatter's `ITEM_SUFFIX_TEMPLATE` (`0x5d8b7f` → `0x703bf0`), read off the VM as
/// `FrameScript_GetText` reads it, into the copy every surface names through
/// ([`crate::items::RandomProperties::suffix_template`]), once the frame's Lua has run: a handler
/// holds no VM to read it at its call, so a script's write reaches the names the next frame's
/// packets compose. Written only on a change, so the name feeds rebuild then alone.
pub(super) fn copy_suffix_template(
    script: Option<NonSend<UiScript>>,
    props: Option<ResMut<crate::items::RandomProperties>>,
) {
    let (Some(script), Some(mut props)) = (script, props) else {
        return;
    };
    let template = script.global_text("ITEM_SUFFIX_TEMPLATE");
    if props.suffix_template.as_deref() != Some(template.as_str()) {
        props.suffix_template = Some(template);
    }
}

/// What an item record's view reads beside the record ([`template_view`]): the spells its
/// trigger lines name and the caster their `$`-tokens read, the bind point's area for `$z`, and
/// the tables its ids are named through.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct RecordViews<'w, 's> {
    spells: Option<Res<'w, crate::ui_action::Spells>>,
    // The caster the `$`-tokens read: its spell modifiers and its skills.
    caster: CasterReads<'w, 's>,
    skill_lines: Option<Res<'w, crate::ui_spellbook::SkillLines>>,
    // The `$z` token: the bind point's area (`SMSG_BINDPOINTUPDATE`), named through `AreaTable`.
    home_bind: Option<Res<'w, crate::net::HomeBind>>,
    places: (
        Option<Res<'w, crate::ui_quest_log::QuestHeaderNamesRes>>,
        Option<Res<'w, benilla_assets::MapCatalogRes>>,
    ),
    factions: Option<Res<'w, crate::target::Factions>>,
    sub_classes: Option<Res<'w, super::ItemSubClasses>>,
    // `GetItemInfo`'s `itemType` and `itemTexture`.
    classes: Option<Res<'w, super::ItemClasses>>,
    icons: Option<Res<'w, ItemDisplays>>,
}

impl RecordViews<'_, '_> {
    /// The bind point's area name, the `$z` token and `GetBindLocation()`.
    fn home_area(&self) -> Option<&str> {
        self.home_bind
            .as_deref()
            .and_then(|b| b.0)
            .and_then(|id| self.places.0.as_deref()?.0.resolve(id as i32))
    }

    /// What the `$`-tokens read of the player: its skills and its gender.
    fn token_reads(&self) -> TokenReads {
        token_reads(self.caster.1.single().ok())
    }

    /// Whether the caster's spell modifiers moved since the last run.
    fn mods_changed(&self) -> bool {
        self.caster.0.is_changed()
    }

    /// Whether `t`'s view reads the player beyond the record: its `$`-tokens, or the standing
    /// label, which reads the gender.
    fn reads_player(&self, t: &ItemInfo) -> bool {
        item_has_mod_tokens(t, self.spells.as_deref()) || t.required_rep_faction != 0
    }

    /// The view of each of `ids` the cache holds, as the getters read it.
    fn build(
        &self,
        items: &Items,
        script: &UiScript,
        ids: impl IntoIterator<Item = u32>,
    ) -> Vec<(u32, benilla_ui::script::ItemTemplateView)> {
        let me = self.caster.1.single().ok();
        let gender = token_reads(me).1;
        let home_area = self.home_area();
        let spell_mods = &*self.caster.0;
        let spell_res = self.spells.as_deref();
        let skill_catalog = self.skill_lines.as_deref().map(|s| &s.catalog);
        let skills = crate::spell::PlayerSkills {
            lines: skill_catalog,
            spells: spell_res.map(|s| &s.catalog),
            items: Some(items),
        };
        let level =
            |d: &benilla_formats::SpellDisplay| crate::spell::player_spell_level(me, skills, d);
        let power_cost = |d: &benilla_formats::SpellDisplay| {
            me.map_or(-1, |s| {
                crate::spell::usable::power_cost(d, s, skills, spell_mods)
            })
        };
        let power_per_second = |d: &benilla_formats::SpellDisplay| {
            let level = crate::spell::SpellLevel::Player { me, skills };
            crate::spell::usable::per_second_cost(d, level)
        };
        let caster = TokenCaster {
            level: &level,
            power_cost: &power_cost,
            power_per_second: &power_per_second,
        };
        let tables = ViewTables {
            skill_lines: skill_catalog,
            factions: self.factions.as_deref().map(|f| f.catalog()),
            sub_classes: self.sub_classes.as_deref().map(|s| &s.0),
            classes: self.classes.as_deref().map(|c| &c.0),
            icons: self.icons.as_deref(),
            areas: self.places.0.as_deref().map(|a| &a.0),
            maps: self.places.1.as_deref().map(|m| &m.0),
        };
        let strings = benilla_ui::strings::Strings::Vm(script.lua());
        ids.into_iter()
            .filter_map(|id| {
                let t = items.template(id, 0, Ask::Read)?;
                let view = template_view(
                    t,
                    spell_res,
                    Some(spell_mods),
                    caster,
                    tables,
                    home_area,
                    &|| gender,
                    strings,
                );
                Some((id, view))
            })
            .collect()
    }
}

/// `ItemCacheReceive` (`0x55bdb0`) stores the answer in its node (`0x55bef6`, the valid byte at
/// `0x55befb`) before it runs a callback (`0x55bf39`), so a callback that reads the cache reads
/// the record: the VM's copy of the record lands here, at its packet, ahead of the callbacks
/// ([`crate::net::objects`]), and a refusal of a held one drops it. The change is the VM's then,
/// which the item-tooltip feed does not push again ([`Items::vm_took`]).
pub(crate) fn push_answered_record(
    In(entry): In<u32>,
    mut vm: crate::script_calls::VmWith<(ResMut<'static, Items>, RecordViews<'static, 'static>)>,
) {
    vm.turn(|script, (mut items, views)| {
        let Some(script) = script else {
            return;
        };
        match views.build(&items, script, [entry]).pop() {
            Some((id, view)) => script.set_item_template(id, view),
            None if !items.template_answered(entry) => return,
            None => script.drop_item_template(entry),
        }
        items.vm_took(entry);
    });
}

/// The quest log's callback (`0x4e0830`) on a found record, at its place in the entry's list
/// ([`crate::net::objects`]): a `QUEST_LOG_UPDATE` for each record the VM's lookups missed
/// (`0x4e083a`, [`UiScript::flush_quest_log_refires`]), held and parked at this place.
pub(crate) fn quest_log_answered(world: &mut World) {
    fn flush(mut vm: crate::script_calls::HostedVm) {
        vm.run(|script| {
            if let Some(script) = script {
                script.flush_quest_log_refires();
            }
        });
    }
    let mut parked = crate::field_callbacks::Parked::new(world);
    parked.leg(world, |world| {
        if let Err(err) = crate::script_calls::run_cached(world, flush) {
            debug!("ui_items: the quest log's record callback did not run: {err}");
        }
    });
}

/// The tooltips' callbacks (`0x52e530`, `0x52f820`) on a found record, at its place in the
/// entry's list ([`crate::net::objects`]): the rebuilds run on the item lane there
/// ([`UiScript::answer_item_tooltips`]).
pub(crate) fn tooltips_answered(world: &mut World, entry: u32) {
    if let Some(mut lane) = world.get_resource_mut::<crate::item_lane::ItemLane>() {
        lane.push(crate::item_watch::Run {
            notice: Some(crate::item_watch::ItemNotice::TooltipRecord { entry }),
            ..Default::default()
        });
    }
}

/// The item-tooltip feed: every template that lands is pushed unprompted, and a new VM is seated
/// with every cached one, so the first hover never misses, as the reference reads one item cache
/// synchronously that a `/reload` keeps. A miss the VM asked for is the
/// getter's lookup with a callback, which queries it here and asks a refused one again; a miss
/// `GetItemInfo` read is served from the cache and never queried (`0x48e172`).
pub(super) fn feed_item_stats(
    mut vm: crate::script_calls::VmWith<(Res<'static, Items>, RecordViews<'static, 'static>)>,
    commands: Res<NetCommands>,
    mut pending: Local<crate::ui_script::VmMemo<std::collections::HashSet<u32>>>,
    // The record changes this VM has taken in ([`Items::changed_since`]).
    mut cursor: Local<crate::ui_script::VmMemo<u64>>,
    mut last_home: Local<crate::ui_script::VmMemo<Option<String>>>,
    mut last_reads: Local<crate::ui_script::VmMemo<Option<TokenReads>>>,
) {
    vm.turn(|script, (items, views)| {
        let Some(script) = script else {
            return;
        };
        let (pending, new_vm) = pending.get_reset(script);
        let cursor = cursor.get(script);
        if new_vm {
            pending.extend(items.cached_template_ids());
            *cursor = items.change_count();
        }
        let last_home = last_home.get(script);
        // The `$`-tokens follow the player's skills and gender.
        let reads = views.token_reads();
        let reads_changed = last_reads.get(script).as_ref() != Some(&reads);
        *last_reads.get(script) = Some(reads);
        // `GetBindLocation()`'s push, here so it and the `$z` token share one name, and ahead of
        // the pending gate below: the bind point can arrive while the feed idles.
        let home_area = views.home_area();
        // Pushed on change, per VM: a rebuilt VM is fed again the frame it appears.
        if last_home.as_deref() != home_area {
            script.set_bind_location(home_area);
            *last_home = home_area.map(str::to_string);
            // Re-substitute every held view, or one pushed before the bind point carries a raw
            // `$z`.
            pending.extend(items.cached_template_ids());
        }

        // A landed record is pushed unprompted; a refusal on a held record removed it from the
        // cache (`0x556f35`), and the VM's copy goes.
        if *cursor != items.change_count() {
            for (id, held) in items.changed_since(*cursor) {
                if held {
                    pending.insert(id);
                } else {
                    pending.remove(&id);
                    script.drop_item_template(id);
                }
            }
            *cursor = items.change_count();
        }
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
        // The views built from the player beyond the template, built again when what they read
        // moves.
        if views.mods_changed() || reads_changed {
            pending.extend(
                items
                    .template_store()
                    .iter()
                    .filter(|(_, t)| views.reads_player(t))
                    .map(|(&id, _)| id),
            );
        }
        if pending.is_empty() {
            return;
        }
        let ready: Vec<u32> = pending
            .iter()
            .copied()
            .filter(|&id| items.template(id, 0, Ask::Read).is_some())
            .collect();
        // Built first, pushed after: the lookup borrows the VM that `set_item_template` needs
        // mutably.
        let built = views.build(&items, script, ready);
        for (id, view) in built {
            pending.remove(&id);
            script.set_item_template(id, view);
        }
        // The quest log's item callbacks (`0x4e0830`) on the records the push carried, once each.
        script.flush_quest_log_refires();
    })
}

/// What the `$`-tokens read of the caster.
type CasterReads<'w, 's> = (
    Res<'w, crate::spell::SpellModifiers>,
    Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
);

/// One bag slot from its item guid: instance, template, icon and use-spell cooldown, the object
/// read off `held` (the item objects as of the getter's place) and its countdowns off `objects`.
/// `None` is an empty slot or one whose item object has not streamed; an occupied one whose
/// template is not yet resolved is `Some` with empty fields. With no cooldown list or clock pair
/// (`cooldown`) the slot carries no cooldown.
#[allow(clippy::too_many_arguments)] // the slot's inputs, each a store it reads
pub(super) fn resolve_slot(
    guid: u64,
    held: &(impl super::ItemObjects + ?Sized),
    objects: &Objects,
    items: &Items,
    icons: Option<&ItemDisplays>,
    rolls: crate::items::RollCatalogs,
    cooldown: Option<(&crate::spell::Cooldowns, std::time::Instant, f64)>,
    spells: Option<&benilla_formats::SpellCatalog>,
    names: &crate::names::NameCache,
    petitions: Option<&crate::ui_petition::PetitionRecords>,
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
    let fields = held.object(guid)?;
    // What the getters read of it as they run, read here the same way.
    let live = super::live::LiveItem::object(fields, guid, items, icons, rolls);
    let (entry, count, durability, readable, creator, flags, already_bound, enchant_lines) = (
        fields.object_entry().unwrap_or(0),
        live.stack_count().unwrap_or(1),
        live.durability(),
        // `GetContainerItemInfo`'s readable flag (`0x4f97ba`).
        live.readable(),
        // The creator's name for "<Made by %s>" and "Written by %s", `None` until the
        // name query answers. The tooltip build asks it in its call
        // ([`benilla_ui::script::RecordAsk::ItemBuildName`]).
        fields
            .item_creator()
            .filter(|&g| g != 0)
            .and_then(|g| names.resolve_player(g, Ask::Read).map(str::to_string)),
        // The tooltip's unlocked (0x4) and wrapped (0x8) bits.
        fields.item_flags().unwrap_or(0),
        // `0x5da2c0`: soulbound, or carrying a binding enchant, off the raw descriptor.
        crate::items::already_bound(
            fields,
            crate::items::record_flags(items, fields.object_entry().unwrap_or(0), guid),
            rolls.enchants,
        ),
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
    let field_duration = fields.item_duration().unwrap_or(0);
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
            uncached_link: live.uncached_link(),
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
            field_duration,
            ..Default::default()
        });
    };
    Some(ContainerSlot {
        uncached_link: None,
        use_fork: super::drain::use_fork(&t, flags, fields.item_text_id().unwrap_or(0)),
        item_class: live.class(),
        bonding: t.bonding,
        use_spell: t.use_spell.map_or(0, |u| u.spell_id),
        // A charter's guild name and master, keyed by the petition id the server puts in
        // enchantment slot 0 (`PetitionsHandler.cpp:126`), where the reference reads it
        // (`0x5ef337`) and its enchant lines skip it (`0x52c9e0`). The tooltip build asks it.
        // The signature count rides beside the id (`0x52ba51`).
        petition: (t.flags & benilla_protocol::messages::ITEM_FLAG_CHARTER != 0)
            .then(|| {
                let id = fields.item_enchant(0).unwrap_or(0);
                let view = petitions?.tooltip_view(id as u32, guid, names, Ask::Read)?;
                Some(benilla_ui::script::PetitionSlotView {
                    signatures: fields.item_petition_signatures(),
                    ..view
                })
            })
            .flatten(),
        texture: live.texture(),
        count,
        durability,
        quality: live.quality(),
        inventory_type: live.inventory_type(),
        item_id: entry,
        // `GetContainerItemLink` links the object (`0x4f997f` → `0x52ae00`); the tooltip reads its
        // name back off it.
        link: live.link(),
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
        field_duration,
        equip_slots: find_equip_slot(t.inventory_type, has_relic_slot),
        bar_placeable: t.placeable_on_action_bar(),
        consumable: t.is_consumable(),
        spell_0: t.has_spell_0(),
        // The tooltip's charges lines (`0x52d9e9`) and the sell price's charge scale
        // (`0x52e43d`) read them (`flds+0x28`).
        spell_charges: std::array::from_fn(|i| fields.item_spell_charges(i as u8).unwrap_or(0)),
        cooldown: cooldown.and_then(|(cooldowns, now, ui_now)| {
            use_cooldown(
                &t,
                entry,
                fields.item_spell_charges(0).unwrap_or(0),
                cooldowns,
                spells,
                now,
                ui_now,
            )
        }),
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
pub(crate) fn use_cooldown(
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

/// [`use_cooldown`] for the item object held under `guid` in `objects`, its record only read
/// (`0x4c8b3a`, `0x4f9a3f`): `None` for no object or no entry, `Some(None)` for the all-zero
/// answer, a record not cached included: the getters' query at the call
/// ([`crate::spell::CooldownLive`]).
pub(crate) fn object_cooldown(
    objects: &(impl super::ItemObjects + ?Sized),
    guid: u64,
    items: &Items,
    cooldowns: &crate::spell::Cooldowns,
    spells: Option<&benilla_formats::SpellCatalog>,
    now: std::time::Instant,
    ui_now: f64,
) -> Option<Option<(i64, u32, bool)>> {
    let fields = objects.object(guid)?;
    let entry = fields.object_entry()?;
    let Some(t) = items.template(entry, guid, Ask::Read) else {
        return Some(None);
    };
    let charges_left = fields.item_spell_charges(0).unwrap_or(0);
    Some(use_cooldown(
        t,
        entry,
        charges_left,
        cooldowns,
        spells,
        now,
        ui_now,
    ))
}

#[cfg(test)]
mod tests {
    use super::resolve_slot;

    /// A set's member names come from the item-name store (`0x52df5a` looks up `0xc0e2dc` with a
    /// callback and a zero owner), asked with `CMSG_ITEM_NAME_QUERY`, never the template store.
    #[test]
    fn a_set_member_is_named_through_the_item_name_store() {
        use crate::item_names::ItemNames;
        use crate::net::{ClientCommand, NetCommands};
        use crate::query_cache::Ask;

        let (tx, rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let mut names = ItemNames::default();
        let members = super::set_members(&[16_866, 16_867], &names, Ask::Event(&commands));
        assert_eq!(members, [(16_866, None), (16_867, None)]);
        let asked = crate::query_cache::queries(&rx);
        assert!(
            matches!(
                asked[..],
                [
                    ClientCommand::ItemNameQuery {
                        entry: 16_866,
                        guid: 0
                    },
                    ClientCommand::ItemNameQuery {
                        entry: 16_867,
                        guid: 0
                    }
                ]
            ),
            "{asked:?}"
        );
        names.file(16_866, Some("Helm of Wrath".into()));
        let members = super::set_members(&[16_866], &names, Ask::Read);
        assert_eq!(members, [(16_866, Some("Helm of Wrath".into()))]);
    }

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
            super::TokenCaster::NONE,
            Default::default(),
            None,
            &|| 0,
            benilla_ui::strings::Strings::Host(&|_| None),
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
            let v = super::template_view(
                t,
                None,
                None,
                super::TokenCaster::NONE,
                tables,
                None,
                &|| 0,
                benilla_ui::strings::Strings::Host(&|_| None),
            );
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
                super::TokenCaster::NONE,
                Default::default(),
                None,
                &move || gender,
                benilla_ui::strings::Strings::Host(&get),
            )
            .required_rep_line
        };
        assert_eq!(line(0).as_deref(), Some("Requires UNKNOWN - Honored"));
        assert_eq!(line(1).as_deref(), Some("Requires UNKNOWN - Honored (f)"));
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
        let petitions = crate::ui_petition::PetitionRecords::default();
        let mut slot = |guid: u64| {
            let o = objs.get();
            resolve_slot(
                guid,
                &o,
                &o,
                &items,
                None,
                RollCatalogs::NONE,
                Some((&cooldowns, std::time::Instant::now(), 0.0)),
                None,
                &names,
                Some(&petitions),
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
        let mut petitions = crate::ui_petition::PetitionRecords::default();
        crate::ui_petition::net::query_response(
            &mut petitions,
            benilla_protocol::messages::PetitionQueryResponse {
                petition_id: 0x51,
                name: "Pact".into(),
                ..Default::default()
            },
        );
        let slot = {
            let o = objs.get();
            resolve_slot(
                0xC1,
                &o,
                &o,
                &items,
                None,
                RollCatalogs::NONE,
                Some((
                    &crate::spell::Cooldowns::default(),
                    std::time::Instant::now(),
                    0.0,
                )),
                None,
                &NameCache::default(),
                Some(&petitions),
                false,
            )
        }
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
        let petitions = crate::ui_petition::PetitionRecords::default();
        let mut seats = |guid: u64| {
            {
                let o = objs.get();
                resolve_slot(
                    guid,
                    &o,
                    &o,
                    &items,
                    None,
                    RollCatalogs::NONE,
                    Some((&cooldowns, std::time::Instant::now(), 0.0)),
                    None,
                    &names,
                    Some(&petitions),
                    false,
                )
            }
            .expect("an occupied slot")
            .occupied_seats
        };
        assert_eq!(seats(0xB1), Some(2));
        assert_eq!(seats(0xB2), Some(0));
        assert_eq!(seats(0xB3), None, "not a container");
    }

    /// `GetContainerItemInfo`'s readable flag (`0x4f97ba`): the item's `+0x74`, `0x5d9e10`, reads
    /// the cached template's PageText (`[rec+0x19c]`), and only a zero falls back to the
    /// instance's `ITEM_FIELD_ITEM_TEXT_ID` (`0x4f97c1`-`0x4f97cf`). A book reads readable; a
    /// template not yet cached reads zero.
    #[test]
    fn a_template_page_text_makes_a_bag_item_readable() {
        use crate::items::{test_template, Items, RollCatalogs};
        use crate::names::NameCache;
        use crate::ui_items::TestObjects;
        use benilla_protocol::field::FIELD_OBJECT_ENTRY;
        use benilla_protocol::ObjectFields;

        let mut objs = TestObjects::new();
        objs.spawn(
            0xD1,
            ObjectFields::from_pairs(&[(FIELD_OBJECT_ENTRY, 8383)]),
        );
        objs.spawn(
            0xD2,
            ObjectFields::from_pairs(&[(FIELD_OBJECT_ENTRY, 2589)]),
        );
        objs.spawn(
            0xD3,
            ObjectFields::from_pairs(&[(FIELD_OBJECT_ENTRY, 9999)]),
        );
        let mut items = Items::default();
        let mut book = test_template("A Dusty Unsent Letter");
        book.page_text = 1931;
        items.insert_template(8383, Some(book));
        items.insert_template(2589, Some(test_template("Linen Cloth")));
        let names = NameCache::default();
        let cooldowns = crate::spell::Cooldowns::default();
        let petitions = crate::ui_petition::PetitionRecords::default();
        let mut readable = |guid: u64| {
            {
                let o = objs.get();
                resolve_slot(
                    guid,
                    &o,
                    &o,
                    &items,
                    None,
                    RollCatalogs::NONE,
                    Some((&cooldowns, std::time::Instant::now(), 0.0)),
                    None,
                    &names,
                    Some(&petitions),
                    false,
                )
            }
            .expect("an occupied slot")
            .readable
        };
        assert!(readable(0xD1), "the template's page text");
        assert!(!readable(0xD2), "no text anywhere");
        assert!(!readable(0xD3), "an uncached template reads zero");
    }

    /// `0x5da2c0` skips its enchant leg for a record flagged `0x2000` (`0x5da2e3`, `0x5da2e6`), a
    /// charter's: a binding enchant id in its slots binds nothing, and only `ITEM_FIELD_FLAGS & 1`
    /// counts.
    #[test]
    fn a_charter_flagged_item_is_not_bound_by_an_enchant() {
        use crate::items::{test_template, Enchants, Items, RollCatalogs};
        use crate::names::NameCache;
        use crate::ui_items::TestObjects;
        use benilla_protocol::field::FIELD_OBJECT_ENTRY;
        use benilla_protocol::ObjectFields;

        /// A `SpellItemEnchantment` row whose `Flags & 1` binds the item.
        const BINDING: u32 = 1900;
        /// `ITEM_FIELD_ENCHANTMENT` slot 1's id.
        const F_ENCHANT_1: u16 = 22 + 3;
        let mut objs = TestObjects::new();
        let enchanted = |entry: u32| {
            ObjectFields::from_pairs(&[(FIELD_OBJECT_ENTRY, entry), (F_ENCHANT_1, BINDING)])
        };
        objs.spawn(0xE1, enchanted(5863));
        objs.spawn(0xE2, enchanted(2589));
        let mut items = Items::default();
        let mut charter = test_template("Guild Charter");
        charter.flags = benilla_protocol::messages::ITEM_FLAG_CHARTER;
        items.insert_template(5863, Some(charter));
        items.insert_template(2589, Some(test_template("Linen Cloth")));
        let enchants = Enchants(benilla_formats::EnchantCatalog::from_rows(
            Default::default(),
            Default::default(),
            [(BINDING, 1)].into(),
        ));
        let rolls = RollCatalogs {
            props: None,
            enchants: Some(&enchants),
        };
        let names = NameCache::default();
        let cooldowns = crate::spell::Cooldowns::default();
        let petitions = crate::ui_petition::PetitionRecords::default();
        let mut bound = |guid: u64| {
            {
                let o = objs.get();
                resolve_slot(
                    guid,
                    &o,
                    &o,
                    &items,
                    None,
                    rolls,
                    Some((&cooldowns, std::time::Instant::now(), 0.0)),
                    None,
                    &names,
                    Some(&petitions),
                    false,
                )
            }
            .expect("an occupied slot")
            .already_bound
        };
        assert!(!bound(0xE1), "the charter's enchant leg is skipped");
        assert!(bound(0xE2), "any other item's binding enchant binds it");
    }

    /// A worn item answers the item cooldown query by its entry (`0x6e2ed0`), read by the live-API
    /// slot id, one past the descriptor slot. An on-use item with none running
    /// answers cold and enabled (`0x6e1406`); one with no use spell is absent, the query leaving
    /// its zeros (`0x6e2f34`).
    #[test]
    fn a_worn_item_on_cooldown_is_keyed_by_its_live_slot_id() {
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

        let worn = worn(&store, &mut objs, &items, &cooldowns, now, 50.0);
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
        let petitions = crate::ui_petition::PetitionRecords::default();
        for (&(guid, ..), want) in cases.iter().zip(expected) {
            let slot = {
                let o = objs.get();
                resolve_slot(
                    guid,
                    &o,
                    &o,
                    &items,
                    None,
                    RollCatalogs::NONE,
                    Some((&cooldowns, now, 50.0)),
                    None,
                    &names,
                    Some(&petitions),
                    false,
                )
            }
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
        let worn = worn(&store, &mut objs, &items, &cooldowns, now, 50.0);
        let want: HashMap<usize, (i64, u32, bool)> = expected
            .iter()
            .enumerate()
            .filter_map(|(i, w)| w.map(|w| (13 + i, w)))
            .collect();
        assert_eq!(worn, want);
    }

    use crate::ui_action::real_spells;
    use benilla_protocol::messages::ItemSpellEntry;
    use std::collections::HashMap;

    /// The worn items' cooldown answers by live-API slot id, as `CooldownQuery::Worn` reads each
    /// at the call (`0x4c8b10` → `0x6228a0`, then `0x6e2ed0`).
    fn worn(
        store: &crate::net::ObjectStore,
        objs: &mut crate::ui_items::TestObjects,
        items: &crate::items::Items,
        cooldowns: &crate::spell::Cooldowns,
        now: std::time::Instant,
        ui_now: f64,
    ) -> HashMap<usize, (i64, u32, bool)> {
        let o = objs.get();
        (0..19u8)
            .filter_map(|slot0| {
                let guid = store.0.player_inv_slot(slot0).filter(|&g| g != 0)?;
                let t = super::object_cooldown(&o, guid, items, cooldowns, None, now, ui_now)
                    .flatten()?;
                Some((usize::from(slot0) + 1, t))
            })
            .collect()
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
    fn modifier_refresh_tracks_only_item_descriptions_with_tokens() {
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
                super::spell_desc_text(
                    Some(&spells),
                    id,
                    super::TokenCaster::NONE,
                    None,
                    &|| 0,
                    None,
                    &no_strings
                ),
                None,
                "spell {id} ({:?}) has no description, so the reference prints no trigger line",
                d.name
            );
        }

        // The control: Fireball (133), described, still yields its line.
        let fireball = super::spell_desc_text(
            Some(&spells),
            133,
            super::TokenCaster::NONE,
            None,
            &|| 0,
            None,
            &no_strings,
        )
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
                super::TokenCaster::NONE,
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

    /// A refusal on a record the cache held removes it (`0x556ec0`-`0x556f37`), so the VM's copy
    /// goes too: its readers, the equip questions among them, miss where they read it before.
    #[test]
    fn a_refusal_on_a_held_record_leaves_the_vm_too() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<Items>()
            .init_resource::<crate::spell::SpellModifiers>()
            .add_systems(Update, super::feed_item_stats);
        app.world_mut()
            .resource_mut::<Items>()
            .insert_template(25, Some(crate::items::test_template("Linen Cloth")));
        app.insert_non_send_resource(benilla_ui::script::UiScript::new().unwrap());
        let name = |app: &mut App| {
            app.world_mut()
                .non_send_resource_mut::<benilla_ui::script::UiScript>()
                .eval::<Option<String>>("return (GetItemInfo(25))")
                .unwrap()
        };
        app.update();
        assert_eq!(name(&mut app).as_deref(), Some("Linen Cloth"));
        app.world_mut()
            .resource_mut::<Items>()
            .insert_template(25, None);
        app.update();
        assert_eq!(name(&mut app), None, "the refusal removed it");
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

    /// The name formatter's template is the VM's `ITEM_SUFFIX_TEMPLATE` (`0x5d8b7f` →
    /// `0x703bf0`), read off the VM into the catalog every surface names through, and read again
    /// when it changes.
    #[test]
    fn the_suffix_template_is_read_off_the_vm() {
        let mut app = App::new();
        app.insert_resource(crate::items::RandomProperties::new(
            benilla_formats::RandomPropertyCatalog::from_rows(std::collections::HashMap::from([(
                862,
                benilla_formats::RandomProperty {
                    suffix: "of the Bear".into(),
                    enchants: [0; 5],
                },
            )])),
        ))
        .add_systems(Update, super::copy_suffix_template);
        let script = benilla_ui::script::UiScript::new().unwrap();
        script.run(r#"ITEM_SUFFIX_TEMPLATE = "%s -- %s""#).unwrap();
        app.insert_non_send_resource(script);
        app.update();
        let name = |app: &App| {
            let props = app.world().resource::<crate::items::RandomProperties>();
            crate::items::item_display_name("Robe", 862, Some(props))
        };
        assert_eq!(name(&app), "Robe -- of the Bear");
        app.world_mut()
            .non_send_resource_mut::<benilla_ui::script::UiScript>()
            .run(r#"ITEM_SUFFIX_TEMPLATE = nil"#)
            .unwrap();
        app.update();
        assert_eq!(name(&app), "", "a missing key formats the empty string");
    }
}
