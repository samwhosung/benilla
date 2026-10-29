//! The Craft window: the client-built book of a craft type (Enchanting, Beast Training), opened by
//! its opener spell with no packet, whose builds and getters read it here as it stands
//! ([`CraftLive`]). Reagents and tools come off `Spell.dbc`, and the description's `$` tokens
//! resolve as a tooltip's do. An enchant is an item-targeted cast (`Targets = 0x10`) whose pick is
//! the ordinary targeting cursor ([`crate::spell::targeting`]).

use bevy::prelude::*;

use benilla_formats::{
    substitute_parts, ExpandAt, SPELL_ATTR_IS_TRADESKILL, SPELL_EFFECT_CREATE_ITEM,
    SPELL_EFFECT_ENCHANT_ITEM, SPELL_EFFECT_ENCHANT_ITEM_TEMPORARY, SPELL_EFFECT_LEARN_PET_SPELL,
    SPELL_EFFECT_LEARN_SPELL,
};
use benilla_protocol::messages::PLAYER_SKILL_SLOTS;
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::{
    CraftDifficulty, CraftReagent, CraftRecipe, CraftState, CraftTooltip, TooltipText,
};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::{NetCommands, NetHandlerApp, ObjectStore, Objects};
use crate::query_cache::Ask;
use crate::spell::known::{PetReads, PetView};
use crate::spell::CastCommit;
use crate::ui_action::{PlayerActions, Spells};
use crate::ui_items::{count_of, item_icon, InventoryScope, ItemObjects};
use crate::ui_spellbook::SkillLines;
use crate::ui_tradeskill::SpellFocus;

/// The open Craft window, client-local. A nonzero opener `EffectMiscValue[0]` routes here
/// (`Spell_C::TryCast` `0x6e4b60`) and is the craft type (1 Beast Training, 3 Enchanting) the
/// client keeps at `0xbdcfb8` for admission and row order. The opener of the open type, the
/// world-enter init (`0x4f5ee0`) and the session end clear it too: a logout's fresh VM never runs
/// the old one's `OnHide`.
#[derive(Resource, Default)]
pub(crate) struct CraftOpen {
    pub(crate) line: Option<u32>,
    pub(crate) craft_type: u32,
}

/// A craft getter's lookup of a record in flight, inside its call
/// ([`benilla_ui::script::ItemCallback::Craft`]): the item cache asks a miss, a refused entry
/// again, and keeps the callback `0x4f7710` on it, one per call, which outlives a VM and rebuilds
/// the list when its record lands (`0x4f75dd`, `0x4f79f2`, `0x4f771a`).
pub(crate) fn ask_item_record(items: &Items, commands: &NetCommands, entry: u32, owner: u64) {
    if items.template(entry, owner, Ask::Event(commands)).is_none() {
        items.wait(entry, crate::items::RecordCallback::Craft);
    }
}

/// `EffectImplicitTargetA`/`B` 5, the pet (`0x4f632a`).
const TARGET_PET: u32 = 5;

pub(crate) struct UiCraftPlugin;

impl Plugin for UiCraftPlugin {
    fn build(&self, app: &mut App) {
        app.net_handler(SessionEventKind::Disconnected, on_session_end);
        app.init_resource::<CraftOpen>();
    }
}

/// The Craft window dies with the session.
fn on_session_end(In(_): In<SessionEvent>, mut open: ResMut<CraftOpen>) {
    *open = CraftOpen::default();
}

/// The line's skill total the Craft window bands difficulty on (`0x5ea520` in the build
/// `0x4f60c0`), where the TradeSkill window uses the raw rank: no slot is 0 (`0x5ea537`), else the
/// rank, plus the permanent bonus when the rank is above 0 (`0x5ea571`), plus the signed temporary
/// bonus, never below 0 (`0x5ea5bc`).
fn skill_total(store: &ObjectStore, skill_id: u32) -> i32 {
    let Some(s) = skill_slot(store, skill_id) else {
        return 0;
    };
    let rank = i32::from(s.value);
    let rank = if rank > 0 {
        rank + i32::from(s.perm_bonus as u16)
    } else {
        rank
    };
    (rank + i32::from(s.temp_bonus)).max(0)
}

/// `GetCraftInfo`'s training-point cost and required level (`0x4f6fe6`-`0x4f7093`), from the
/// pet bar's unit (`0xb714a0`): each effect slot that is `LEARN_SPELL` aimed at the pet
/// (`+0x148` 5) or `LEARN_PET_SPELL` (`0x4f702e`-`0x4f7037`) names a learned spell; its ability
/// row's `reqtrainpoints`, written only when found (`0x4f7049`), and its required level,
/// written only when `0x60c840` answers. The last such slot wins; no pet unit is 0 and 0.
fn cost_and_level(
    pet: &PetView,
    d: &benilla_formats::SpellDisplay,
    objects: &impl ItemObjects,
) -> (u32, i32) {
    let Some(unit) = pet.unit(objects, pet.bar_guid) else {
        return (0, 0);
    };
    let (mut cost, mut level) = (0, 0);
    for i in 0..3 {
        let effect = d.effects[i];
        let pet_learn = (effect == SPELL_EFFECT_LEARN_SPELL
            && d.effect_implicit_target_a[i] == TARGET_PET)
            || effect == SPELL_EFFECT_LEARN_PET_SPELL;
        if !pet_learn {
            continue;
        }
        let taught = d.effect_trigger_spell[i];
        if let Some((_, row)) = pet.ability(&unit, taught) {
            cost = row.train_points;
        }
        if let Some(l) = pet.required_level(&unit, taught) {
            level = l;
        }
    }
    (cost, level)
}

/// A craft row's bucket (`0x4f61a8`-`0x4f63cc`). A missing ability row or a trivial high of 0
/// leaves 0, `none` (`0x4f61dd`, `0x4f61ea`). Otherwise the ladder on the skill total of the row's
/// line (`0x5ea520`), compared signed: a low of 0 is the high less 25, negative below 25
/// (`0x4f61fc`-`0x4f6205`), and the mid is their sum halved toward zero (`0x4f622b`). A row still
/// at 0 is `used` (`0x4f62f7`) when its first `LEARN_SPELL` (`0x4f6310`) teaches a spell the
/// player knows (`0x4f6347` → `0x4f6360`), or, aimed at the pet (either target 5, `0x4f6323`,
/// `0x4f6333`), one the unit `UNIT_FIELD_SUMMON` names knows or has a higher rank of (`0x4f6369`-
/// `0x4f63b7` → `0x4f63cc`).
fn craft_bucket(
    d: &benilla_formats::SpellDisplay,
    sla: Option<&benilla_formats::SlaInfo>,
    store: &ObjectStore,
    known: &std::collections::BTreeSet<u32>,
    pet: Option<&PetView>,
    objects: &impl ItemObjects,
) -> CraftDifficulty {
    if let Some(sla) = sla.filter(|a| a.trivial_high != 0) {
        let high = sla.trivial_high as i32;
        let low = match sla.trivial_low as i32 {
            0 => high - 25,
            low => low,
        };
        let total = skill_total(store, sla.skill_id);
        return if total < low {
            CraftDifficulty::Optimal
        } else if total < (low + high) / 2 {
            CraftDifficulty::Medium
        } else if total < high {
            CraftDifficulty::Easy
        } else {
            CraftDifficulty::Trivial
        };
    }
    let Some(i) = (0..3).find(|&i| d.effects[i] == SPELL_EFFECT_LEARN_SPELL) else {
        return CraftDifficulty::None;
    };
    let taught = d.effect_trigger_spell[i];
    let at_pet =
        d.effect_implicit_target_a[i] == TARGET_PET || d.effect_implicit_target_b[i] == TARGET_PET;
    let used = if !at_pet {
        known.contains(&taught)
    } else {
        pet.zip(store.0.unit_summon())
            .and_then(|(pet, guid)| Some((pet, pet.unit(objects, guid)?)))
            .is_some_and(|(pet, unit)| {
                pet.knows(&unit, taught) || pet.knows_higher_rank(&unit, taught)
            })
    };
    if used {
        CraftDifficulty::Used
    } else {
        CraftDifficulty::None
    }
}

/// The first slot holding `skill_id` (`0x5ea3f0`).
fn skill_slot(
    store: &ObjectStore,
    skill_id: u32,
) -> Option<benilla_protocol::messages::PlayerSkillSlot> {
    (0..PLAYER_SKILL_SLOTS)
        .filter_map(|i| store.0.player_skill(i))
        .find(|s| u32::from(s.skill_id) == skill_id)
}

/// `GetCraftDisplaySkillLine`'s answer (`0x4f6cb0`), for the Enchanting type alone (`0x4f6ce3`):
/// the line of the first spell of the player's type-3 list (`0x4f6d0c`) by its ability row
/// (`0x4f6d14`, `+0x04`, 0 failing at `0x4f6d26`), named by `SkillLine.dbc` (`0x4f6d61`), with the
/// slot's rank and maximum, each plus the permanent bonus word when above 0 (`0x4f6d9d`,
/// `0x4f6e10`). The list is in learn order (`0x5e9e0f`), so the first is the first learned. A
/// player with no slot for the line gives the slot -1, which the reads do not test (`0x4f6d52`):
/// the words 12 bytes below the first slot, `PLAYER_XP` for the rank and maximum and
/// `PLAYER_NEXT_LEVEL_XP`'s high word for the bonus.
fn display_line(
    first: Option<u32>,
    skill_lines: &SkillLines,
    store: &ObjectStore,
) -> Option<(String, u32, u32)> {
    let (race, class) = store.race_class();
    let line = skill_lines.catalog.ability(first?, race, class)?.skill_id;
    if line == 0 {
        return None;
    }
    let name = skill_lines.catalog.line(line)?.name.clone();
    let (rank, max, perm) = match skill_slot(store, line) {
        Some(s) => (
            u32::from(s.value),
            u32::from(s.max),
            u32::from(s.perm_bonus as u16),
        ),
        None => {
            let xp = store.0.player_xp().unwrap_or(0);
            let next = store.0.player_next_level_xp().unwrap_or(0);
            (xp & 0xffff, xp >> 16, next >> 16)
        }
    };
    let with_bonus = |v: u32| if v > 0 { v + perm } else { v };
    Some((name, with_bonus(rank), with_bonus(max)))
}

/// Law D, the row icon: always the recipe's own `SpellIconID`, never the product. `GetCraftIcon`
/// (`0x4f7160`-`0x4f7204`) never reads `EffectItemType[0]` (`+0x19c`), so a rod recipe fronts the
/// spell's icon here and the rod's in the TradeSkill window.
fn craft_icon(d: &benilla_formats::SpellDisplay) -> Option<String> {
    d.icon.clone()
}

/// The tooltip (`SetCraftSpell` `0x533e90`), off the recipe's own effects: the first slot that
/// is `LEARN_SPELL` names its trigger spell, unchecked, or `CREATE_ITEM` its item; otherwise the
/// recipe itself, as for every enchant. Unlike the trainer's law it never tests
/// `LEARN_PET_SPELL` and never falls through on an unresolvable trigger.
fn craft_tooltip(spell_id: u32, d: &benilla_formats::SpellDisplay) -> CraftTooltip {
    for i in 0..3 {
        if d.effects[i] == SPELL_EFFECT_LEARN_SPELL {
            return CraftTooltip::Spell(d.effect_trigger_spell[i]);
        }
        if d.effects[i] == SPELL_EFFECT_CREATE_ITEM {
            return CraftTooltip::Item(d.effect_item_type[i]);
        }
    }
    CraftTooltip::Spell(spell_id)
}

/// `GetCraftDescription`'s text (`0x4f7a90`) for a row of `open_type`: the spell's own
/// description when it is not empty (`0x4f7b21`-`0x4f7b2f`), expanded with the pet flag set for
/// craft type 1 (`0x4f7b3d`, `0x4f7b53`); else the description of the spell the first
/// `LEARN_SPELL` or `LEARN_PET_SPELL` effect teaches, when it has a record (`0x4f7b71`-
/// `0x4f7ba3`), expanded with the pet flag set for 57 (`0x4f7bce`, `0x4f7be6`); else `None`, the
/// nil (`0x4f7bae`). Each template a token names is left to the getter's read of the VM's
/// strings; the expansion is the spell tooltip's (level 0, stack 1, `ExpandAt::default`).
fn craft_description(
    d: &benilla_formats::SpellDisplay,
    spells: &benilla_formats::SpellCatalog,
    open_type: u32,
    (player, pet): crate::spell::TokenContexts<'_, '_>,
) -> Option<TooltipText> {
    let expand = |text: &str, d: &benilla_formats::SpellDisplay, at_pet: bool| {
        let ctx = if at_pet { pet } else { player };
        crate::ui_tooltip::tooltip_text(substitute_parts(text, d, ctx, ExpandAt::default()))
    };
    if let Some(text) = d.description.as_deref().filter(|t| !t.is_empty()) {
        return Some(expand(text, d, open_type == CRAFT_TYPE_BEAST_TRAINING));
    }
    (0..3).find_map(|i| {
        let effect = d.effects[i];
        if !matches!(
            effect,
            SPELL_EFFECT_LEARN_SPELL | SPELL_EFFECT_LEARN_PET_SPELL
        ) {
            return None;
        }
        let taught = spells.get(d.effect_trigger_spell[i])?;
        let text = taught.description.as_deref().unwrap_or("");
        Some(expand(text, taught, effect == SPELL_EFFECT_LEARN_PET_SPELL))
    })
}

/// Beast Training's craft type, which sets the expander's pet flag for a row's own text
/// (`cmp esi,1` at `0x4f7b3d`).
const CRAFT_TYPE_BEAST_TRAINING: u32 = 1;

/// What the craft list's build and getters read (`0x4f60c0`, `0x4f6c10`-`0x4f7be6`): the
/// player's known spells and lists, the catalogs, the player object and the bags as of the drain's
/// place ([`crate::ui_unit::views::ViewStores`]), the item cache, the pet and the bind point.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct CraftReads<'w, 's> {
    pet: PetReads<'w>,
    actions: Res<'w, PlayerActions>,
    spells: Option<Res<'w, Spells>>,
    spell_mods: Res<'w, crate::spell::SpellModifiers>,
    skill_lines: Option<Res<'w, SkillLines>>,
    focus: Option<Res<'w, SpellFocus>>,
    icons: Option<Res<'w, ItemDisplays>>,
    /// The `$z` token: the bind point's area (`0x5ed420` at `0x507f07`), named through
    /// `AreaTable` (`0x507f2b`).
    home_bind: Option<Res<'w, crate::net::HomeBind>>,
    area_names: Option<Res<'w, crate::ui_quest_log::QuestHeaderNamesRes>>,
    objects: Objects<'w, 's>,
    stores: crate::ui_unit::views::ViewStores<'w, 's>,
    items: Res<'w, Items>,
}

impl CraftReads<'_, '_> {
    /// The active player's store as of here (`0x468460`).
    fn player(&self) -> Option<&ObjectStore> {
        self.objects.player().and_then(|e| self.stores.get(e).ok())
    }

    /// The bind point's area name, `None` before it resolves, where `$z` reads `HOME_INN`.
    fn home_area(&self) -> Option<&str> {
        self.home_bind
            .as_deref()
            .and_then(|b| b.0)
            .and_then(|id| self.area_names.as_deref()?.0.resolve(id as i32))
    }

    /// `f` over the `$`-token contexts of the player `store` and of its pet, the two casters the
    /// expander's pet flag picks between for its level and cost terms (`0x6e3130`, `0x6e31b0`,
    /// `0x6e3300`): the pet is the charm, else the summon, as of here.
    fn with_contexts<R>(
        &self,
        spells: &Spells,
        skill_lines: &SkillLines,
        store: &ObjectStore,
        f: impl FnOnce(crate::spell::TokenContexts<'_, '_>) -> R,
    ) -> R {
        let pet = store
            .0
            .unit_pet_guid()
            .and_then(|guid| self.stores.by_guid(guid, self.objects.store(guid)));
        let skills = crate::spell::PlayerSkills {
            lines: Some(&skill_lines.catalog),
            spells: Some(&spells.catalog),
            items: Some(&self.items),
        };
        crate::spell::with_token_contexts(
            spells,
            skills,
            &self.spell_mods,
            (Some(store), pet),
            self.home_area(),
            f,
        )
    }

    /// The book of `craft_type`, opened by an opener of `line`, as a build reads it; `None` while
    /// a catalog or our player is missing. The list only reads the item cache: the build asks
    /// nothing (`0x4f60c0`), and a reagent or a totem is asked by its getter's call, inside it
    /// (`0x4f75f2`, `0x4f7a0a`).
    pub(crate) fn state(&self, line: u32, craft_type: u32) -> Option<CraftState> {
        let spells = self.spells.as_deref()?;
        let skill_lines = self.skill_lines.as_deref()?;
        let store = self.player()?;
        let name = skill_lines
            .catalog
            .line(line)
            .map(|l| l.name.clone())
            .unwrap_or_else(|| format!("Skill {line}"));
        let listed = self.listed(spells, craft_type);
        let (display_line, rank, max_rank) = (craft_type == 3)
            .then(|| display_line(listed.first().copied(), skill_lines, store))
            .flatten()
            .map_or((None, 0, 0), |(n, r, m)| (Some(n), r, m));
        let recipes: Vec<CraftRecipe> = self.with_contexts(spells, skill_lines, store, |ctxs| {
            listed
                .iter()
                .filter_map(|&s| self.recipe(s, spells, skill_lines, store, craft_type, ctxs))
                .collect()
        });
        // No sort here: the engine orders the rows by the craft type's own comparator
        // (`benilla_ui::script::craft::recipe_order`).
        Some(CraftState {
            display_line,
            name,
            rank,
            max_rank,
            craft_type,
            recipes,
        })
    }

    /// The build's book of the open window: `Some(None)` with no type open or no player, as the
    /// build's leaves (`0x4f60e3`, `0x4f6109`); `None` while a catalog is missing.
    fn book(&self, open: &CraftOpen) -> Option<Option<CraftState>> {
        self.spells.as_deref()?;
        self.skill_lines.as_deref()?;
        Some(open.line.and_then(|line| self.state(line, open.craft_type)))
    }

    /// The player's list of `craft_type` (`CGPlayer_C + 0x1cd0 + 0x10*type`, `0x5ea330`), in learn
    /// order (`0x5e9e0f`): admitted at the learn (`0x5e9c20`) when known, not a profession spell
    /// (`Attributes & 0x20`) and `castUI` equal to the type; `SkillLineAbility` decides no
    /// membership.
    fn listed(&self, spells: &Spells, craft_type: u32) -> Vec<u32> {
        self.actions
            .craft_order
            .iter()
            .copied()
            .filter(|&s| {
                spells.catalog.get(s).is_some_and(|d| {
                    d.cast_ui == craft_type && d.attributes & SPELL_ATTR_IS_TRADESKILL == 0
                })
            })
            .collect()
    }

    /// `spell_id`'s row as its getters read it at the call, `open_type` the window's type
    /// (`[0xbdcfb8]`, 0 while closed) for the description's pet flag; `None` declines, with a
    /// catalog, our player or the spell's record missing.
    fn recipe_now(&self, spell_id: u32, open_type: u32) -> Option<CraftRecipe> {
        let spells = self.spells.as_deref()?;
        let skill_lines = self.skill_lines.as_deref()?;
        let store = self.player()?;
        self.with_contexts(spells, skill_lines, store, |ctxs| {
            self.recipe(spell_id, spells, skill_lines, store, open_type, ctxs)
        })
    }

    /// `GetCraftName`'s answer for `craft_type` (`0x4f6c10`): the name of the spell in the
    /// player's effect slot of the type (`0x5ea360`), which a learn writes by the spell's misc
    /// value (`0x5e9e1c`-`0x5e9e36`) and an unlearn leaves; `Some(None)` with no player or no such
    /// spell, `None` while the spells are missing.
    fn name_now(&self, craft_type: u32) -> Option<Option<String>> {
        let spells = self.spells.as_deref()?;
        Some(self.player().and_then(|_| {
            let slot = (*self
                .actions
                .effect_slots
                .get(usize::try_from(craft_type).ok()?)?)?;
            spells.catalog.get(slot).map(|d| d.name.clone())
        }))
    }

    /// `GetCraftDisplaySkillLine`'s answer past its type test (`0x4f6ce3`); `Some(None)` with no
    /// player, `None` while a catalog is missing.
    fn display_line_now(&self) -> Option<Option<(String, u32, u32)>> {
        let spells = self.spells.as_deref()?;
        let skill_lines = self.skill_lines.as_deref()?;
        Some(self.player().and_then(|store| {
            display_line(self.listed(spells, 3).first().copied(), skill_lines, store)
        }))
    }

    /// One row of the list (`0x4f6146`-`0x4f63cc`) beside what its getters read: the bucket and
    /// count the build stores, and the reagents, tools, cost, level and description the getters
    /// read at their calls. `open_type` sets the description's pet flag (`0x4f7b31`).
    fn recipe(
        &self,
        s: u32,
        spells: &Spells,
        skill_lines: &SkillLines,
        store: &ObjectStore,
        open_type: u32,
        ctxs: crate::spell::TokenContexts<'_, '_>,
    ) -> Option<CraftRecipe> {
        let d = spells.catalog.get(s)?;
        let objects = self.stores.item_objects(&self.objects);
        let pet = self.pet.view(&skill_lines.catalog, &spells.catalog);
        // A spell with no ability row keeps its row, in group 0 (`0x4f61b6`).
        let (race, class) = store.race_class();
        let sla = skill_lines.catalog.ability(s, race, class);
        let mut reagents = Vec::new();
        let mut num_available = u32::MAX;
        for &(entry, need) in d.reagents.iter().filter(|&&(e, n)| e != 0 && n != 0) {
            let have = count_of(&store.0, &objects, entry, InventoryScope::DEFAULT);
            // A reagent's icon is the shared item chain (`0x5d88b0`) through its display.
            let (name, icon) = match self.items.template(entry, 0, Ask::Read) {
                Some(t) => (
                    Some(t.name.clone()),
                    item_icon(self.icons.as_deref(), t.display_info_id),
                ),
                None => (None, None),
            };
            reagents.push(CraftReagent {
                item: entry,
                name,
                icon,
                need,
                have,
            });
            num_available = num_available.min(have / need);
        }
        if reagents.is_empty() {
            num_available = 0;
        }
        // Focus first, then the totems (`0x4ff980`), the list `GetCraftSpellFocus` (`0x4f78b0`)
        // returns. The focus's flag is a literal `1.0`, never reddened.
        let mut tools = Vec::new();
        if d.requires_spell_focus != 0 {
            if let Some(n) = self
                .focus
                .as_deref()
                .and_then(|f| f.catalog.name(d.requires_spell_focus))
            {
                tools.push((n.to_string(), true));
            }
        }
        let mut tools_pending = Vec::new();
        for &t in d.totems.iter().filter(|&&t| t != 0) {
            let have = count_of(&store.0, &objects, t, InventoryScope::DEFAULT) > 0;
            match self.items.template(t, 0, Ask::Read) {
                Some(info) => tools.push((info.name.clone(), have)),
                None => tools_pending.push(t),
            }
        }
        let needs_item_target = matches!(
            d.effects[0],
            SPELL_EFFECT_ENCHANT_ITEM | SPELL_EFFECT_ENCHANT_ITEM_TEMPORARY
        );
        let (training_point_cost, required_level) = pet
            .as_ref()
            .map_or((0, 0), |p| cost_and_level(p, d, &objects));
        Some(CraftRecipe {
            spell_id: s,
            name: d.name.clone(),
            sub_name: d.rank.clone().unwrap_or_default(),
            difficulty: craft_bucket(d, sla, store, &self.actions.spells, pet.as_ref(), &objects),
            num_available,
            icon: craft_icon(d),
            description: craft_description(d, &spells.catalog, open_type, ctxs),
            needs_item_target,
            reagents,
            tools,
            tooltip: craft_tooltip(s, d),
            // `spellLevel` at `+0x74` (column 29), not `baseLevel` (column 28).
            spell_level: d.spell_level,
            // The group key is the ability row's line (`0x4f61b1`), 0 with none.
            group: sla.map_or(0, |a| a.skill_id),
            skill_line: sla
                .and_then(|a| skill_lines.catalog.line(a.skill_id))
                .map(|l| l.name.clone()),
            training_point_cost,
            required_level,
            tools_pending,
        })
    }
}

/// The craft getters' and builds' reads as they run, the call host's half of
/// [`benilla_ui::script::CraftBookQuery`], [`benilla_ui::script::CraftRecipeQuery`],
/// [`benilla_ui::script::CraftNameQuery`] and [`benilla_ui::script::CraftDisplayLineQuery`], over
/// the open type `[0xbdcfb8]`.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct CraftLive<'w, 's> {
    open: Res<'w, CraftOpen>,
    reads: CraftReads<'w, 's>,
}

impl crate::script_calls::live::LiveFamily for CraftLive<'_, '_> {
    fn answer(&mut self, ask: &mut benilla_ui::script::live::LiveAsk<'_>) -> bool {
        use benilla_ui::script::{
            CraftBookQuery, CraftDisplayLineQuery, CraftNameQuery, CraftRecipeQuery,
        };
        let (open, reads) = (&*self.open, &self.reads);
        // The type a close zeroes (`0x4f5fe0`).
        let open_type = open.line.map_or(0, |_| open.craft_type);
        ask.answer(|_: &CraftBookQuery| reads.book(open))
            || ask.answer(|q: &CraftRecipeQuery| reads.recipe_now(q.0, open_type))
            || ask.answer(|q: &CraftNameQuery| reads.name_now(q.0))
            || ask.answer(|_: &CraftDisplayLineQuery| reads.display_line_now())
    }
}

/// `DoCraft(index)` past its row (`0x4f7d10`): the row's cast through `CastSpellAsActivePlayer`
/// (`0x4f7d64` → `0x6e5a90`) inside the call, with no item and no target. The cast ladder takes
/// it from there: an enchant's `Targets = 0x10` arms the cursor's item half, a rod craft commits
/// at once. The row test is the VM's, against the list it holds; the reference tests no open
/// marker.
pub(crate) fn do_craft(cast: &mut crate::spell::ScriptCast, spell_id: u32) {
    debug!("ui_craft: DoCraft({spell_id})");
    let ctx = cast.targeting.context();
    cast.ladder.send(spell_id, &ctx, CastCommit::Spell);
}

/// The craft window's close inside its call ([`crate::script_calls::ScriptCallHost`]).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct CraftSteps<'w> {
    /// Absent in a world with no craft window, which drops the call.
    open: Option<ResMut<'w, CraftOpen>>,
}

impl CraftSteps<'_> {
    /// `CloseCraft()` (`0x4f6c00` → `0x4f5fe0`): the open marker `[0xbdcfb8]` zeroed, no packet.
    pub(crate) fn close(&mut self) {
        if let Some(open) = self.open.as_deref_mut() {
            debug!("ui_craft: client-side close (no packet)");
            open.line = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::SelfPlayer;
    use benilla_formats::SpellDisplay;
    use benilla_ui::script::UiScript;

    /// `CloseCraft` signals `CRAFT_CLOSE` in the verb (`0x4f5fef`) and nothing signals a second
    /// close a frame later; the opener's toggle signals its own (`0x4f6017`).
    #[test]
    fn a_verbs_close_is_signalled_once() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<CraftOpen>()
            .init_resource::<PlayerActions>()
            .init_resource::<crate::spell::SpellModifiers>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<Items>()
            .insert_resource(NetCommands(tx))
            .insert_resource(crate::ui_action::Spells::empty_for_tests())
            .insert_resource(SkillLines {
                catalog: benilla_formats::SkillLineCatalog::from_spell_lines([]),
            });
        app.world_mut()
            .spawn((ObjectStore(Default::default()), SelfPlayer));
        let mut script = UiScript::new().unwrap();
        script.keep_calls();
        script
            .run(
                "CLOSES = 0 local f = CreateFrame('Frame') f:RegisterEvent('CRAFT_CLOSE') \
                 f:SetScript('OnEvent', function() CLOSES = CLOSES + 1 end)",
            )
            .unwrap();
        app.insert_non_send_resource(script);
        let closes = |app: &App| {
            app.world()
                .non_send_resource::<UiScript>()
                .eval::<i64>("return CLOSES")
                .unwrap()
        };

        toggle(&mut app, 333, 3);
        app.update();
        crate::script_calls::with_hosted_vm(app.world_mut(), |vm| {
            vm.expect("a VM").run("CloseCraft()")
        })
        .unwrap();
        assert_eq!(closes(&app), 1, "the verb's own signal");
        // The host's half, which this harness has none of.
        app.world_mut().resource_mut::<CraftOpen>().line = None;
        app.update();
        assert_eq!(closes(&app), 1, "no second signal");

        toggle(&mut app, 333, 3);
        app.update();
        toggle(&mut app, 333, 3);
        app.update();
        assert_eq!(closes(&app), 2, "the opener's close signals in its toggle");
    }

    #[test]
    fn law_d_is_always_the_spells_own_icon_even_for_a_rod_recipe() {
        let rod = SpellDisplay {
            name: "Runed Copper Rod".into(),
            icon: Some("SPELL".into()),
            effects: [SPELL_EFFECT_CREATE_ITEM, 0, 0],
            effect_item_type: [6218, 0, 0],
            ..Default::default()
        };
        assert_eq!(craft_icon(&rod), Some("SPELL".into()));
    }

    /// The binding pushes nil; there is no item arm to fall through to.
    #[test]
    fn law_d_is_nil_when_the_spell_carries_no_icon() {
        let d = SpellDisplay {
            effect_item_type: [6218, 0, 0],
            ..Default::default()
        };
        assert_eq!(craft_icon(&d), None);
    }
    #[test]
    fn craft_tooltip_reads_the_recipes_own_effects() {
        // An enchant has no matching slot: the recipe spell itself.
        let enchant = SpellDisplay {
            effects: [SPELL_EFFECT_ENCHANT_ITEM, 0, 0],
            ..Default::default()
        };
        assert_eq!(craft_tooltip(7420, &enchant), CraftTooltip::Spell(7420));

        // A rod craft: the item of the same slot.
        let rod = SpellDisplay {
            effects: [SPELL_EFFECT_CREATE_ITEM, 0, 0],
            effect_item_type: [6218, 0, 0],
            ..Default::default()
        };
        assert_eq!(craft_tooltip(7421, &rod), CraftTooltip::Item(6218));

        // A `LEARN_SPELL` slot hops to its trigger without checking that it resolves.
        let mut teacher = SpellDisplay::default();
        teacher.effects[1] = SPELL_EFFECT_LEARN_SPELL;
        teacher.effect_trigger_spell[1] = 999_999;
        assert_eq!(craft_tooltip(5149, &teacher), CraftTooltip::Spell(999_999));
    }

    /// `0x4f62cc`/`0x4f76b2` count a reagent with `0x622130` and `0x4ffb10` finds a tool with
    /// `0x622270`, both at mask 0: a copy worn in a slot or in the keyring counts, one in the bank
    /// does not.
    #[test]
    fn a_recipe_counts_a_worn_copy_and_the_keyring_but_not_the_bank() {
        use benilla_protocol::field::{FIELD_ITEM_STACK_COUNT, FIELD_OBJECT_ENTRY};
        use benilla_protocol::ObjectFields;
        const DUST: u32 = 10_940;
        const ROD: u32 = 6_218;
        const SPELL: u32 = 7_000;
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        let mut actions = PlayerActions::default();
        actions.spells.insert(SPELL);
        actions.profession_order.push(SPELL);
        actions.craft_order.push(SPELL);
        let mut items = Items::default();
        items.insert_template(DUST, Some(crate::items::test_template("Strange Dust")));
        items.insert_template(ROD, Some(crate::items::test_template("Runed Rod")));
        app.init_resource::<CraftOpen>()
            .insert_resource(actions)
            .init_resource::<crate::spell::SpellModifiers>()
            .init_resource::<crate::net::GuidIndex>()
            .insert_resource(items)
            .insert_resource(NetCommands(tx))
            .insert_resource(crate::ui_action::Spells {
                catalog: benilla_formats::SpellCatalog::from_displays(
                    [(
                        SPELL,
                        SpellDisplay {
                            name: "Enchant".into(),
                            cast_ui: 3,
                            reagents: [
                                (DUST, 2),
                                (0, 0),
                                (0, 0),
                                (0, 0),
                                (0, 0),
                                (0, 0),
                                (0, 0),
                                (0, 0),
                            ],
                            totems: [ROD, 0],
                            ..Default::default()
                        },
                    )]
                    .into_iter()
                    .collect(),
                ),
                ..crate::ui_action::Spells::empty_for_tests()
            })
            .insert_resource(SkillLines {
                catalog: benilla_formats::SkillLineCatalog::from_abilities([(
                    SPELL,
                    benilla_formats::SlaInfo {
                        skill_id: 333,
                        req_skill_value: 1,
                        forward_spell_id: 0,
                        trivial_low: 1,
                        trivial_high: 2,
                    },
                )]),
            });
        for (guid, entry, n) in [
            (0x4000_0000_0000_0001u64, DUST, 2u32),
            (0x4000_0000_0000_0002, ROD, 1),
        ] {
            crate::items::test_spawn_item(
                app.world_mut(),
                guid,
                ObjectFields::from_pairs(&[
                    (FIELD_OBJECT_ENTRY, entry),
                    (FIELD_ITEM_STACK_COUNT, n),
                ]),
                false,
            );
        }
        let player = app
            .world_mut()
            .spawn((SelfPlayer, ObjectStore(Default::default())))
            .id();
        app.insert_non_send_resource(UiScript::new().unwrap());
        // Where the dust and the rod sit: player slots, 17 worn, 81 the keyring, 39 the bank;
        // each read opens the window again, whose build counts them.
        let mut read = |dust_slot: u16, rod_slot: u16| -> (i64, bool) {
            let seat = |slot: u16, guid: u64| {
                let lo = 486 + 2 * slot;
                [(lo, guid as u32), (lo + 1, (guid >> 32) as u32)]
            };
            app.world_mut()
                .entity_mut(player)
                .insert(ObjectStore(ObjectFields::from_pairs(
                    &[
                        seat(dust_slot, 0x4000_0000_0000_0001),
                        seat(rod_slot, 0x4000_0000_0000_0002),
                    ]
                    .concat(),
                )));
            reopen(&mut app, 333, 3);
            app.world()
                .non_send_resource::<UiScript>()
                .eval::<(i64, bool)>(
                    "local _, _, _, have = GetCraftReagentInfo(1, 1) \
                     local _, hasTool = GetCraftSpellFocus(1) return have, hasTool == 1",
                )
                .unwrap()
        };
        assert_eq!(read(23, 24), (2, true), "the backpack, as before");
        assert_eq!(read(17, 18), (2, true), "worn copies");
        assert_eq!(read(81, 82), (2, true), "the keyring");
        assert_eq!(read(39, 40), (0, false), "the bank is outside mask 0");
    }

    /// A craft description's `$z` reads the bind area like every spell text (`0x507f07`-
    /// `0x507f45`): the area's name once the bind point resolves, the `HOME_INN` text before.
    #[test]
    fn a_craft_description_names_the_bind_area_or_the_inn() {
        const SPELL: u32 = 7_001;
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        let mut actions = PlayerActions::default();
        actions.spells.insert(SPELL);
        actions.profession_order.push(SPELL);
        actions.craft_order.push(SPELL);
        app.init_resource::<CraftOpen>()
            .insert_resource(actions)
            .init_resource::<crate::spell::SpellModifiers>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<Items>()
            .insert_resource(NetCommands(tx))
            .insert_resource(crate::ui_action::Spells {
                catalog: benilla_formats::SpellCatalog::from_displays(
                    [(
                        SPELL,
                        SpellDisplay {
                            name: "Enchant".into(),
                            cast_ui: 3,
                            description: Some("Returns you to $z.".into()),
                            ..Default::default()
                        },
                    )]
                    .into_iter()
                    .collect(),
                ),
                ..crate::ui_action::Spells::empty_for_tests()
            })
            .insert_resource(SkillLines {
                catalog: benilla_formats::SkillLineCatalog::from_abilities([(
                    SPELL,
                    benilla_formats::SlaInfo {
                        skill_id: 333,
                        req_skill_value: 1,
                        forward_spell_id: 0,
                        trivial_low: 1,
                        trivial_high: 2,
                    },
                )]),
            });
        app.world_mut()
            .spawn((SelfPlayer, ObjectStore(Default::default())));
        let script = UiScript::new().unwrap();
        script.run("HOME_INN = 'your inn'").unwrap();
        app.insert_non_send_resource(script);
        let described = |app: &mut App| {
            reopen(app, 333, 3);
            app.world()
                .non_send_resource::<UiScript>()
                .eval::<String>("return GetCraftDescription(1)")
                .unwrap()
        };
        assert_eq!(described(&mut app), "Returns you to your inn.");
        app.insert_resource(crate::net::HomeBind(Some(12)));
        app.insert_resource(crate::ui_quest_log::QuestHeaderNamesRes(
            benilla_formats::QuestHeaderNames::from_zones([(12, "Elwynn Forest")]),
        ));
        assert_eq!(described(&mut app), "Returns you to Elwynn Forest.");
    }

    #[test]
    fn the_session_end_closes_the_craft_window() {
        let mut app = App::new();
        app.add_plugins(UiCraftPlugin);
        {
            let mut open = app.world_mut().resource_mut::<CraftOpen>();
            open.line = Some(333);
            open.craft_type = 3;
        }

        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![benilla_protocol::SessionEvent::Disconnected {
                reason: "socket".into(),
                end: benilla_protocol::SessionEnd::Lost,
            }],
        );

        let open = app.world().resource::<CraftOpen>();
        assert_eq!((open.line, open.craft_type), (None, 0));
    }

    const BEAST_TRAINING: u32 = 261;
    const ENCHANTING: u32 = 333;
    const TAILORING: u32 = 197;
    const PET_WOLF: u32 = 208;
    const PET_GENERIC: u32 = 270;
    const WOLF_FAMILY: u32 = 1;
    const WOLF_ENTRY: u32 = 299;
    const PET: u64 = 0xF140_0000_0000_0042;
    const OTHER_PET: u64 = 0xF140_0000_0000_0043;
    /// `UNIT_FIELD_SUMMON`, the guid pair the `used` test fetches its unit by (`[fields + 8]`).
    const F_SUMMON: u16 = 8;
    const F_TYPE: u16 = 2;
    const F_ENTRY: u16 = 3;
    const F_XP: u16 = 716;
    const F_NEXT_XP: u16 = 717;
    const F_SKILL_1: u16 = 718;

    /// A learn spell of craft type `cast_ui`: `effects[i]` (36 `LEARN_SPELL`, 57 `LEARN_PET_SPELL`)
    /// teach `triggers[i]`, aimed at `targets[i]` (5 the pet).
    fn learner(
        name: &str,
        rank: &str,
        cast_ui: u32,
        effects: [u32; 3],
        targets: [u32; 3],
        triggers: [u32; 3],
    ) -> SpellDisplay {
        SpellDisplay {
            name: name.into(),
            rank: Some(rank.into()),
            cast_ui,
            effects,
            effect_implicit_target_a: targets,
            effect_trigger_spell: triggers,
            spell_level: 10,
            ..Default::default()
        }
    }

    /// A pet spell the learn effects teach, at `spell_level`.
    fn pet_spell(name: &str, spell_level: u32) -> SpellDisplay {
        SpellDisplay {
            name: name.into(),
            spell_level,
            ..Default::default()
        }
    }

    /// An ability row on `line` with the trivial ranks `(low, high)`.
    fn ability(line: u32, low: u32, high: u32) -> benilla_formats::SlaInfo {
        benilla_formats::SlaInfo {
            skill_id: line,
            req_skill_value: 0,
            forward_spell_id: 0,
            trivial_low: low,
            trivial_high: high,
        }
    }

    /// The window's reads over `displays`, the player knowing `known`, with no pet; [`seat_pet`]
    /// adds one.
    fn craft_feed(
        displays: Vec<(u32, SpellDisplay)>,
        known: &[u32],
        catalog: benilla_formats::SkillLineCatalog,
        player: &[(u16, u32)],
    ) -> App {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        let mut actions = PlayerActions::default();
        actions.spells.extend(known.iter().copied());
        actions.profession_order.extend(known.iter().copied());
        actions.craft_order.extend(known.iter().copied());
        app.init_resource::<CraftOpen>()
            .insert_resource(actions)
            .init_resource::<crate::spell::SpellModifiers>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<Items>()
            .init_resource::<crate::ui_pet::PetBar>()
            .init_resource::<crate::names::NameCache>()
            .insert_resource(crate::ui_pet_stats::PetFamilyTables {
                families: benilla_formats::CreatureFamilies::from_skill_lines([(
                    WOLF_FAMILY,
                    [PET_WOLF, PET_GENERIC],
                )]),
                foods: Default::default(),
            })
            .insert_resource(NetCommands(tx))
            .insert_resource(crate::ui_action::Spells {
                catalog: benilla_formats::SpellCatalog::from_displays(
                    displays.into_iter().collect(),
                ),
                ..crate::ui_action::Spells::empty_for_tests()
            })
            .insert_resource(SkillLines { catalog });
        app.insert_non_send_resource(rx);
        app.world_mut().spawn((
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(player)),
            SelfPlayer,
        ));
        let mut script = UiScript::new().unwrap();
        script.keep_calls();
        app.insert_non_send_resource(script);
        app
    }

    /// [`craft_feed`] with the window of `craft_type` opened as its opener's cast opens it.
    fn craft_feed_open(
        displays: Vec<(u32, SpellDisplay)>,
        known: &[u32],
        catalog: benilla_formats::SkillLineCatalog,
        craft_type: u32,
        player: &[(u16, u32)],
    ) -> App {
        let mut app = craft_feed(displays, known, catalog, player);
        open_type(&mut app, craft_type);
        app
    }

    /// The window of `craft_type` toggled as an opener's cast toggles it (`0x4f6000`), its
    /// signals fired.
    fn open_type(app: &mut App, craft_type: u32) {
        let line = if craft_type == 3 {
            ENCHANTING
        } else {
            BEAST_TRAINING
        };
        toggle(app, line, craft_type);
    }

    /// The window of `craft_type` closed if open and opened again, whose build reads the world as
    /// it stands.
    fn reopen(app: &mut App, line: u32, craft_type: u32) {
        if app.world().resource::<CraftOpen>().line.is_some() {
            toggle(app, line, craft_type);
        }
        toggle(app, line, craft_type);
    }

    /// [`crate::ui_tradeskill::TradeSkillSteps::toggle_craft`] on `app`'s VM.
    fn toggle(app: &mut App, line: u32, craft_type: u32) {
        use bevy::ecs::system::RunSystemOnce;
        app.world_mut()
            .run_system_once(
                move |mut steps: crate::ui_tradeskill::TradeSkillSteps,
                      mut script: NonSendMut<UiScript>| {
                    steps.toggle_craft(&mut script, line, craft_type);
                },
            )
            .expect("the toggle runs");
    }

    /// A wolf at `PET` with `book` in its spellbook, on the pet bar as `bar`, and the player's
    /// `UNIT_FIELD_SUMMON` at `summon`.
    fn seat_pet(app: &mut App, summon: u64, bar: u64, book: &[u32]) {
        let world = app.world_mut();
        let pet = world
            .spawn(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (F_TYPE, 0x9),
                (F_ENTRY, WOLF_ENTRY),
            ])))
            .id();
        world
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(PET, pet);
        world
            .resource_mut::<crate::names::NameCache>()
            .insert_creature(
                WOLF_ENTRY,
                Some(crate::names::CreatureRecord {
                    name: "Wolf".into(),
                    subname: None,
                    creature_type: 1,
                    pet_family: WOLF_FAMILY,
                    rank: 0,
                    type_flags: 0,
                    pet_spell_list: 0,
                    civilian: false,
                    racial_leader: false,
                    display_id: 0,
                }),
            );
        let mut bar_res = world.resource_mut::<crate::ui_pet::PetBar>();
        bar_res.spells.pet_guid = bar;
        bar_res.spells.spells = book
            .iter()
            .map(|&s| benilla_protocol::messages::PetActionEntry { packed: s })
            .collect();
        let mut q = world.query_filtered::<&mut ObjectStore, With<SelfPlayer>>();
        let mut me = q.single_mut(world).unwrap();
        me.0 = benilla_protocol::ObjectFields::from_pairs(&[
            (F_SUMMON, summon as u32),
            (F_SUMMON + 1, (summon >> 32) as u32),
        ]);
    }

    /// Each row's `(name rank, craftType, trainingPointCost, requiredLevel)`, in display order.
    fn craft_rows(app: &mut App) -> Vec<(String, String, i64, i64)> {
        app.world()
            .non_send_resource::<UiScript>()
            .eval::<Vec<Vec<String>>>(
                "local t = {} for i = 1, GetNumCrafts() do \
                 local n, s, k, _, _, tp, l = GetCraftInfo(i) \
                 t[i] = { n .. ' ' .. s, k, tostring(tp), tostring(l) } end return t",
            )
            .unwrap()
            .into_iter()
            .map(|r| {
                (
                    r[0].clone(),
                    r[1].clone(),
                    r[2].parse().unwrap(),
                    r[3].parse().unwrap(),
                )
            })
            .collect()
    }

    /// `0x4f61a8`-`0x4f61e4`: a spell `0x60c7c0` finds no ability row for keeps its row, group 0
    /// and bucket 0, `none`; `GetCraftSkillLine` names no line for group 0.
    #[test]
    fn a_craft_spell_with_no_ability_row_keeps_its_row_at_group_zero_and_bucket_none() {
        let mut app = craft_feed_open(
            vec![(
                24_533,
                learner(
                    "Great Stamina",
                    "Rank 1",
                    1,
                    [36, 0, 0],
                    [0; 3],
                    [4195, 0, 0],
                ),
            )],
            &[24_533],
            benilla_formats::SkillLineCatalog::from_abilities([]),
            1,
            &[],
        );
        assert_eq!(
            craft_rows(&mut app),
            [("Great Stamina Rank 1".into(), "none".into(), 0, 0)]
        );
        assert!(app
            .world()
            .non_send_resource::<UiScript>()
            .eval::<bool>("return GetCraftSkillLine(1) == nil")
            .unwrap());
    }

    /// Great Stamina's ranks as shipped: 4195 and 4196 teach the pet 4187 and 4188, which
    /// `Pet - Generic` chains (`+0x20`).
    fn beast_training() -> (Vec<(u32, SpellDisplay)>, benilla_formats::SkillLineCatalog) {
        let displays = vec![
            (
                4195,
                learner(
                    "Great Stamina",
                    "Rank 1",
                    1,
                    [36, 0, 0],
                    [5, 0, 0],
                    [4187, 0, 0],
                ),
            ),
            (
                4196,
                learner(
                    "Great Stamina",
                    "Rank 2",
                    1,
                    [36, 0, 0],
                    [5, 0, 0],
                    [4188, 0, 0],
                ),
            ),
            (
                9000,
                learner("Track", "Rank 1", 1, [36, 0, 0], [0; 3], [9001, 0, 0]),
            ),
            (
                9100,
                learner("Bond", "Rank 1", 1, [57, 0, 0], [0; 3], [4187, 0, 0]),
            ),
            (
                9200,
                learner("Twin", "Rank 1", 1, [36, 36, 0], [5, 5, 0], [4187, 4188, 0]),
            ),
            (
                9300,
                learner(
                    "Stray",
                    "Rank 1",
                    1,
                    [36, 36, 0],
                    [5, 5, 0],
                    [4188, 7777, 0],
                ),
            ),
            (4187, pet_spell("Great Stamina", 4)),
            (4188, pet_spell("Great Stamina", 8)),
            (9001, pet_spell("Track", 1)),
        ];
        let catalog = benilla_formats::SkillLineCatalog::from_abilities(
            [4195, 4196, 9000, 9100, 9200, 9300].map(|s| (s, ability(BEAST_TRAINING, 0, 0))),
        )
        .with_line_abilities([
            (
                PET_GENERIC,
                4187,
                benilla_formats::LineAbility {
                    superseded_by: 4188,
                    train_points: 5,
                },
            ),
            (
                PET_GENERIC,
                4188,
                benilla_formats::LineAbility {
                    superseded_by: 0,
                    train_points: 10,
                },
            ),
        ]);
        (displays, catalog)
    }

    /// `0x4f61ea`: an ability row whose trivial high is 0, every Beast Training row's, leaves
    /// bucket 0, `none`. A bucket-0 row's first `LEARN_SPELL` (`0x4f6310`) not aimed at the pet
    /// is `used` when the player knows its spell (`0x4f6347` → `0x4f6360`); aimed at the pet
    /// (`EffectImplicitTargetA` 5, `0x4f6323`), when the unit `UNIT_FIELD_SUMMON` names knows it
    /// or a higher rank (`0x4f63a1`, `0x4f63b7` → `0x4f63cc`), which only the pet bar's unit can
    /// (`0x60c77a`): its book. Bucket 5 sorts last (`0x4f6a34`).
    #[test]
    fn beast_training_rows_read_none_and_used_once_known() {
        let known = [4195, 4196, 9000, 9001];
        // The window opens over the pet as it stands: the build reads the bucket (`0x4f6369`).
        let feed = |pet: Option<(u64, u64, &[u32])>| {
            let (displays, catalog) = beast_training();
            let mut app = craft_feed(displays, &known, catalog, &[]);
            if let Some((summon, bar, book)) = pet {
                seat_pet(&mut app, summon, bar, book);
            }
            open_type(&mut app, 1);
            app
        };
        let kinds = |app: &mut App| -> Vec<(String, String)> {
            craft_rows(app)
                .into_iter()
                .filter(|r| r.0.starts_with("Great") || r.0.starts_with("Track"))
                .map(|r| (r.0, r.1))
                .collect()
        };
        let pair = |n: &str, k: &str| (n.to_string(), k.to_string());

        let mut app = feed(None);
        assert_eq!(
            kinds(&mut app),
            [
                pair("Great Stamina Rank 1", "none"),
                pair("Great Stamina Rank 2", "none"),
                pair("Track Rank 1", "used"),
            ],
            "no pet: only the player's own learn reads used"
        );

        let mut app = feed(Some((PET, PET, &[4187])));
        assert_eq!(
            kinds(&mut app),
            [
                pair("Great Stamina Rank 2", "none"),
                pair("Great Stamina Rank 1", "used"),
                pair("Track Rank 1", "used"),
            ],
            "the pet knows rank 1"
        );

        let mut app = feed(Some((PET, PET, &[4188])));
        assert_eq!(
            kinds(&mut app),
            [
                pair("Great Stamina Rank 1", "used"),
                pair("Great Stamina Rank 2", "used"),
                pair("Track Rank 1", "used"),
            ],
            "rank 2 is a higher rank of rank 1"
        );

        let mut app = feed(Some((OTHER_PET, PET, &[4187, 4188])));
        assert_eq!(
            kinds(&mut app),
            [
                pair("Great Stamina Rank 1", "none"),
                pair("Great Stamina Rank 2", "none"),
                pair("Track Rank 1", "used"),
            ],
            "a summon that is not the pet bar's unit knows nothing"
        );
    }

    /// `0x4f6fe6`-`0x4f7093`: with the pet bar's unit (`0xb714a0`), each effect slot that is 36
    /// aimed at the pet (`+0x148` 5) or 57 names a learned spell (`+0xc0`); its first ability row
    /// on the pet family's lines (`0x60c7c0` over `CreatureFamily` `+0x14`, `+0x18`) gives the cost
    /// (`+0x38`, written only when found, `0x4f7049`) and `0x60c840` the level, the learned spell's
    /// `+0x74` (no `SkillRaceClassInfo` row for a pet line raises it). The last such slot wins; no
    /// pet answers 0 and 0.
    #[test]
    fn a_pet_learn_row_answers_its_training_point_cost_and_required_level() {
        let known = [4195, 4196, 9000, 9100, 9200, 9300];
        let feed = |pet: bool| {
            let (displays, catalog) = beast_training();
            let mut app = craft_feed(displays, &known, catalog, &[]);
            if pet {
                seat_pet(&mut app, PET, PET, &[]);
            }
            open_type(&mut app, 1);
            app
        };
        let costs = |app: &mut App| -> Vec<(String, i64, i64)> {
            let mut rows: Vec<_> = craft_rows(app)
                .into_iter()
                .map(|r| (r.0, r.2, r.3))
                .collect();
            rows.sort();
            rows
        };
        let row = |n: &str, tp: i64, l: i64| (n.to_string(), tp, l);
        let mut app = feed(false);
        assert!(
            costs(&mut app).iter().all(|r| (r.1, r.2) == (0, 0)),
            "no pet"
        );
        let mut app = feed(true);
        assert_eq!(
            costs(&mut app),
            [
                row("Bond Rank 1", 5, 4),
                row("Great Stamina Rank 1", 5, 4),
                row("Great Stamina Rank 2", 10, 8),
                row("Stray Rank 1", 10, 8),
                row("Track Rank 1", 0, 0),
                row("Twin Rank 1", 10, 8),
            ]
        );
    }

    /// `GetCraftDisplaySkillLine` (`0x4f6cb0`) reads the slot of its line's rank and maximum, each
    /// plus the permanent bonus word (`+0x852`) when above 0 (`0x4f6d9d`, `0x4f6e10`). A player
    /// with no slot for the line gets slot -1, untested (`0x4f6d52`), so the words 12 bytes below
    /// the first slot: `PLAYER_XP` for the rank and maximum, `PLAYER_NEXT_LEVEL_XP`'s high word
    /// for the bonus.
    #[test]
    fn the_display_line_reads_its_slot_with_the_permanent_bonus() {
        let displays = || {
            vec![(
                7418,
                SpellDisplay {
                    name: "Enchant Bracer - Minor Health".into(),
                    cast_ui: 3,
                    ..Default::default()
                },
            )]
        };
        let catalog = || {
            benilla_formats::SkillLineCatalog::from_abilities([(7418, ability(ENCHANTING, 1, 70))])
                .with_line_names([(ENCHANTING, "Enchanting")])
        };
        let triple = |app: &mut App| {
            app.world()
                .non_send_resource::<UiScript>()
                .eval::<(Option<String>, i64, i64)>("return GetCraftDisplaySkillLine()")
                .unwrap()
        };
        let slot = |value: u32, max: u32, perm: u32| {
            [
                (F_SKILL_1, ENCHANTING),
                (F_SKILL_1 + 1, value | max << 16),
                (F_SKILL_1 + 2, perm << 16),
            ]
        };
        let mut app = craft_feed_open(displays(), &[7418], catalog(), 3, &slot(100, 150, 10));
        assert_eq!(triple(&mut app), (Some("Enchanting".into()), 110, 160));
        let mut app = craft_feed_open(displays(), &[7418], catalog(), 3, &slot(0, 150, 10));
        assert_eq!(triple(&mut app), (Some("Enchanting".into()), 0, 160));
        let mut app = craft_feed_open(
            displays(),
            &[7418],
            catalog(),
            3,
            &[(F_XP, 25 | 50 << 16), (F_NEXT_XP, 3 << 16)],
        );
        assert_eq!(triple(&mut app), (Some("Enchanting".into()), 28, 53));
    }

    /// The display line is the line of the type's first spell (`0x4f6cb0`), the first the player
    /// learned of its list (`0x5e9e0f` appends), not the lowest id.
    #[test]
    fn the_display_line_follows_the_first_spell_learned() {
        let enchant = |name: &str| SpellDisplay {
            name: name.into(),
            cast_ui: 3,
            ..Default::default()
        };
        let displays = vec![
            (7418, enchant("Enchant Bracer - Minor Health")),
            (7420, enchant("Enchant Chest - Minor Health")),
        ];
        let catalog = benilla_formats::SkillLineCatalog::from_abilities([
            (7418, ability(ENCHANTING, 1, 70)),
            (7420, ability(TAILORING, 1, 70)),
        ])
        .with_line_names([(ENCHANTING, "Enchanting"), (TAILORING, "Tailoring")]);
        let app = craft_feed_open(displays, &[7420, 7418], catalog, 3, &[]);
        let name = app
            .world()
            .non_send_resource::<UiScript>()
            .eval::<Option<String>>("return (GetCraftDisplaySkillLine())")
            .unwrap();
        assert_eq!(name.as_deref(), Some("Tailoring"));
    }

    /// `GetCraftDescription` (`0x4f7a90`) falls back from an empty text of the row's own
    /// (`0x4f7b2a`, `0x4f7b2f`) to the text of the spell its first `LEARN_SPELL` or
    /// `LEARN_PET_SPELL` effect teaches (`0x4f7b71`-`0x4f7ba3`, expanded at `0x4f7be6`), and with
    /// neither answers nil (`0x4f7bae`).
    #[test]
    fn a_craft_description_falls_back_to_the_taught_spells_text() {
        let displays = vec![
            (
                9100,
                learner("Bond", "Rank 1", 1, [57, 0, 0], [0; 3], [4187, 0, 0]),
            ),
            (
                9000,
                learner("Track", "Rank 1", 1, [0, 0, 0], [0; 3], [0, 0, 0]),
            ),
            (
                4187,
                SpellDisplay {
                    description: Some("Taunts the target.".into()),
                    ..pet_spell("Great Stamina", 4)
                },
            ),
        ];
        let catalog = benilla_formats::SkillLineCatalog::from_abilities(
            [9100, 9000].map(|s| (s, ability(BEAST_TRAINING, 0, 0))),
        );
        let app = craft_feed_open(displays, &[9100, 9000], catalog, 1, &[]);
        let described: Vec<(String, Option<String>)> = app
            .world()
            .non_send_resource::<UiScript>()
            .eval::<Vec<Vec<String>>>(
                "local t = {} for i = 1, GetNumCrafts() do \
                 t[i] = { (GetCraftInfo(i)), GetCraftDescription(i) or 'nil' } end return t",
            )
            .unwrap()
            .into_iter()
            .map(|r| (r[0].clone(), (r[1] != "nil").then(|| r[1].clone())))
            .collect();
        assert_eq!(
            described,
            [
                ("Bond".into(), Some("Taunts the target.".into())),
                ("Track".into(), None),
            ]
        );
    }

    /// The window's open runs the build (`0x4f6036` → `0x4f60c0`), whose tail signals
    /// `CRAFT_UPDATE` (`0x4f661a`), then `CRAFT_SHOW` (`0x4f6040`); a change between rebuilds, as a
    /// skill's rank, which no watcher rebuilds the craft list on (`0x5de434` is the profession
    /// window's alone), signals nothing, and the row keeps the bucket its build stored
    /// (`0x4f623f`), which `GetCraftInfo` reads (`0x4f6fbc`).
    #[test]
    fn the_open_signals_update_then_show_and_a_bare_change_signals_nothing() {
        let displays = vec![(
            7418,
            SpellDisplay {
                name: "Enchant Bracer - Minor Health".into(),
                cast_ui: 3,
                ..Default::default()
            },
        )];
        let catalog =
            benilla_formats::SkillLineCatalog::from_abilities([(7418, ability(ENCHANTING, 1, 70))]);
        let mut app = craft_feed(
            displays,
            &[7418],
            catalog,
            &[(F_SKILL_1, ENCHANTING), (F_SKILL_1 + 1, 1 | 75 << 16)],
        );
        app.world()
            .non_send_resource::<UiScript>()
            .run(
                "HEARD = {} local f = CreateFrame('Frame') f:RegisterEvent('CRAFT_SHOW') \
                 f:RegisterEvent('CRAFT_UPDATE') \
                 f:SetScript('OnEvent', function() table.insert(HEARD, event) end)",
            )
            .unwrap();
        let heard = |app: &mut App| {
            app.update();
            app.world()
                .non_send_resource::<UiScript>()
                .eval::<Vec<String>>("local h = HEARD HEARD = {} return h")
                .unwrap()
        };
        open_type(&mut app, 3);
        assert_eq!(heard(&mut app), ["CRAFT_UPDATE", "CRAFT_SHOW"]);
        let world = app.world_mut();
        let mut q = world.query_filtered::<&mut ObjectStore, With<SelfPlayer>>();
        q.single_mut(world).unwrap().0 = benilla_protocol::ObjectFields::from_pairs(&[
            (F_SKILL_1, ENCHANTING),
            (F_SKILL_1 + 1, 70 | 75 << 16),
        ]);
        assert!(heard(&mut app).is_empty());
        assert_eq!(
            crate::script_calls::with_hosted_vm(app.world_mut(), |s| s
                .expect("a VM")
                .eval::<String>("local _, _, k = GetCraftInfo(1) return k"))
            .unwrap(),
            "medium",
            "the build's bucket, at rank 1 of 1-70"
        );
    }

    /// `SMSG_PET_SPELLS` rebuilds the list on every arrival (`0x4bd990` → `0x4bdafa`), a book
    /// resent unchanged included; a learn or an unlearn of a craft spell rebuilds it once per packet
    /// (`0x5e9e12`, `0x5ea240`), whether or not the list moved; a record a getter's call waited on
    /// rebuilds it once per call (`0x4f7710`).
    /// The window's show looks no item up: the reference's list build asks nothing (`0x4f60c0`),
    /// and a reagent or a totem is asked by its getter's call alone (`0x4f75f2`, `0x4f7a0a`).
    /// The reagents the link getters read, which pass no callback (`0x4f780a`), are read off the
    /// cache, never asked.
    #[test]
    fn the_show_asks_no_reagent_or_totem() {
        const DUST: u32 = 10_940;
        const ROD: u32 = 6218;
        let displays = vec![(
            7418,
            SpellDisplay {
                name: "Enchant Bracer - Minor Health".into(),
                cast_ui: 3,
                reagents: [
                    (DUST, 1),
                    (0, 0),
                    (0, 0),
                    (0, 0),
                    (0, 0),
                    (0, 0),
                    (0, 0),
                    (0, 0),
                ],
                totems: [ROD, 0],
                ..Default::default()
            },
        )];
        let catalog =
            benilla_formats::SkillLineCatalog::from_abilities([(7418, ability(ENCHANTING, 1, 70))]);
        let mut app = craft_feed_open(displays, &[7418], catalog, 3, &[]);
        app.update();
        app.update();
        assert_eq!(
            app.world()
                .non_send_resource::<UiScript>()
                .eval::<i64>("return GetNumCrafts()")
                .unwrap(),
            1,
            "the window is up"
        );
        let rx = app
            .world()
            .non_send_resource::<crossbeam_channel::Receiver<crate::net::ClientCommand>>();
        let asked: Vec<_> = crate::query_cache::queries(rx)
            .into_iter()
            .filter(|c| matches!(c, crate::net::ClientCommand::ItemQuery { .. }))
            .collect();
        assert!(asked.is_empty(), "{asked:?}");
        let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
        assert!(script.take_item_stat_asks().is_empty(), "no pre-ask");
    }

    /// The pet book and a learn or an unlearn of a craft spell rebuild the open list once per
    /// packet (`0x4bdafa`, `0x5e9e12`, `0x5ea240`); an unlearn the list does not hold, nothing.
    #[test]
    fn each_book_packet_signals_one_update() {
        const DUST: u32 = 10_940;
        let displays = vec![
            (
                7418,
                SpellDisplay {
                    name: "Enchant Bracer - Minor Health".into(),
                    cast_ui: 3,
                    reagents: [
                        (DUST, 1),
                        (0, 0),
                        (0, 0),
                        (0, 0),
                        (0, 0),
                        (0, 0),
                        (0, 0),
                        (0, 0),
                    ],
                    ..Default::default()
                },
            ),
            (
                7420,
                SpellDisplay {
                    name: "Enchant Chest - Minor Health".into(),
                    cast_ui: 3,
                    ..Default::default()
                },
            ),
        ];
        let catalog =
            benilla_formats::SkillLineCatalog::from_abilities([(7418, ability(ENCHANTING, 1, 70))]);
        let mut app = craft_feed(displays, &[7418], catalog, &[]);
        app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
        crate::ui_tradeskill::register_rebuild_triggers(&mut app);
        app.world()
            .non_send_resource::<UiScript>()
            .run(
                "HEARD = {} local f = CreateFrame('Frame') f:RegisterEvent('CRAFT_SHOW') \
                 f:RegisterEvent('CRAFT_UPDATE') \
                 f:SetScript('OnEvent', function() table.insert(HEARD, event) end)",
            )
            .unwrap();
        let heard = |app: &mut App| {
            app.update();
            app.world()
                .non_send_resource::<UiScript>()
                .eval::<Vec<String>>("local h = HEARD HEARD = {} return h")
                .unwrap()
        };
        open_type(&mut app, 3);
        assert_eq!(heard(&mut app), ["CRAFT_UPDATE", "CRAFT_SHOW"]);
        let pet_spells = || SessionEvent::PetSpells(Box::default());
        crate::net::handlers::dispatch(app.world_mut(), vec![pet_spells(), pet_spells()]);
        assert_eq!(heard(&mut app), ["CRAFT_UPDATE", "CRAFT_UPDATE"]);
        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![
                SessionEvent::SpellLearned { spell_id: 7418 },
                SessionEvent::SpellRemoved { spell_id: 7418 },
            ],
        );
        assert_eq!(heard(&mut app), ["CRAFT_UPDATE", "CRAFT_UPDATE"]);
        // An unlearn the type's list does not hold skips the rebuild (`0x5ea1c9`).
        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![SessionEvent::SpellRemoved { spell_id: 7420 }],
        );
        assert!(heard(&mut app).is_empty());
    }

    /// The world-enter init `0x4f5ee0` (`0x4909c4`) zeroes the craft type (`[0xbdcfb8]`) and
    /// signals nothing; `CRAFT_CLOSE` is `0x4f5fe0`'s alone. A worldport's cascade closes the
    /// window under the interface that survives it with no close, and a reload's closes it under
    /// the new one, which hears no show.
    #[test]
    fn the_world_enter_init_closes_the_window_silently() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<CraftOpen>()
            .init_resource::<PlayerActions>()
            .init_resource::<crate::spell::SpellModifiers>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<Items>()
            .insert_resource(NetCommands(tx))
            .insert_resource(crate::ui_action::Spells::empty_for_tests())
            .insert_resource(SkillLines {
                catalog: benilla_formats::SkillLineCatalog::from_spell_lines([]),
            });
        app.world_mut()
            .spawn((ObjectStore(Default::default()), SelfPlayer));
        let listening = || {
            let script = UiScript::new().unwrap();
            script
                .run(
                    "HEARD = {} local f = CreateFrame('Frame') f:RegisterEvent('CRAFT_SHOW') \
                     f:RegisterEvent('CRAFT_UPDATE') f:RegisterEvent('CRAFT_CLOSE') \
                     f:SetScript('OnEvent', function() table.insert(HEARD, event) end)",
                )
                .unwrap();
            script
        };
        app.insert_non_send_resource(listening());
        let world_enter = |app: &mut App| {
            app.world_mut()
                .run_system_cached(crate::ui_tradeskill::reset_crafts_on_world_enter)
                .expect("the craft inits run");
        };
        let heard = |app: &mut App| {
            app.update();
            app.world()
                .non_send_resource::<UiScript>()
                .eval::<Vec<String>>("local h = HEARD HEARD = {} return h")
                .unwrap()
        };
        let open = |app: &mut App| {
            toggle(app, 333, 3);
            heard(app)
        };

        assert_eq!(open(&mut app), ["CRAFT_UPDATE", "CRAFT_SHOW"]);
        world_enter(&mut app);
        assert!(heard(&mut app).is_empty(), "a worldport closes it silently");
        assert_eq!(app.world().resource::<CraftOpen>().line, None);

        assert_eq!(open(&mut app), ["CRAFT_UPDATE", "CRAFT_SHOW"]);
        app.insert_non_send_resource(listening());
        world_enter(&mut app);
        assert!(heard(&mut app).is_empty(), "a reload shows nothing");
        assert_eq!(app.world().resource::<CraftOpen>().line, None);
    }
}
