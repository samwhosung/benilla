//! The trainer feed: [`TrainerOpen`] from `SMSG_TRAINER_LIST`, [`feed_trainer`] resolving it into
//! Lua [`TrainerService`]s and firing the trainer events, and [`TrainerSteps`] sending the buys
//! and running `CloseTrainer`'s close core inside their calls. A
//! bought row repaints through the re-evaluator ([`reeval`]), never through
//! `SMSG_TRAINER_BUY_SUCCEEDED`, and a refusal goes only to the log.

use std::collections::BTreeSet;

use benilla_formats::{
    LearnEffect, SkillLineCatalog, SpellCatalog, TokenContext, SPELL_EFFECT_LEARN_PET_SPELL,
    SPELL_EFFECT_SKILL_STEP,
};
use benilla_protocol::messages::{trainer_spell_state, TrainerSpell};
use bevy::prelude::*;

use benilla_ui::script::{
    TrainerAbilityReq, TrainerService, TrainerServiceCategory, TrainerSkillReq, TrainerState,
    TrainerTooltip, UiScript, TRAINER_GROUP_KNOWN,
};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::names::NameCache;
use benilla_protocol::field::{
    FIELD_PLAYER_FIELD_COINAGE, FIELD_PLAYER_SKILL_INFO_1_1, FIELD_UNIT_LEVEL,
};
use benilla_protocol::messages::PLAYER_SKILL_SLOTS;
use bevy::ecs::system::SystemParam;

use crate::net::{
    ClientCommand, FieldChanged, Guid, GuidIndex, NetCommands, ObjectStore, SelfPlayer,
};
use crate::query_cache::Ask;
use crate::ui_action::{PlayerActions, Spells};
use crate::ui_script::{SweepApp, SweepLeg, UiFeed};
use crate::ui_session::{CoreCloses, LatchApp, LatchedWindow, NpcWindow};
use crate::ui_spellbook::SkillLines;

/// `SMSG_TRAINER_LIST`'s `trainer_type` for a tradeskill trainer (0 class, 1 mount, 2 tradeskill,
/// 3 pet), which picks the group key, the icon and the row order.
const TRAINER_TYPE_TRADESKILL: u32 = 2;
/// A mount trainer, which the client calls "talent" (`IsTalentTrainer 0x4d8ed0`).
const TRAINER_TYPE_MOUNT: u32 = 1;
/// A pet trainer, whose list `SetPetGuid` asks again on a new pet (`0x4bc872`).
const TRAINER_TYPE_PET: u32 = 3;

/// The open trainer's cell when it is a pet trainer: `SetPetGuid`'s test, the cell nonzero and the
/// type 3 (`0x4bc860`-`0x4bc879`).
pub(crate) fn open_pet_trainer(open: Option<&TrainerOpen>) -> Option<u64> {
    let open = open?;
    open.trainer
        .filter(|_| open.trainer_type == TRAINER_TYPE_PET)
}

/// `TalkToTrainer` (`0x5df680`) on `trainer`: the window closed through the close core
/// (`0x5df6bb` → `0x4d7440` with a zero guid), then `CMSG_TRAINER_LIST` for it (`0x5df6fc`). The
/// asked guid it remembers (`0xc4d788`), which the list's open tests (`0x5dcc85`), has no cell
/// here: every list opens.
pub(crate) fn talk_to_trainer(world: &mut World, trainer: u64) {
    close_core(world);
    if let Some(net) = world.get_resource::<NetCommands>() {
        let _ = net.0.send(ClientCommand::TrainerList { trainer });
    }
}

/// The open trainer as `SMSG_TRAINER_LIST` delivered it.
#[derive(Resource, Default)]
pub(crate) struct TrainerOpen {
    pub(crate) trainer: Option<u64>,
    /// In wire order, which is the 1-based display order.
    pub(crate) services: Vec<TrainerSpell>,
    pub(crate) trainer_type: u32,
    pub(crate) greeting: String,
    /// A list landed and the engine's filter, collapse set and selection are not yet reset, as
    /// the reference's builder resets them per list (`0x4d7b42` for the selection); an open window
    /// repaints through [`reeval`], never a second list.
    pub(crate) fresh_list: bool,
    /// The passes of `0x4d7d40` owed, one per trigger of the reference's twelve that fired with
    /// the window open: each re-derives, sorts and fires `TRAINER_UPDATE` even if no state moved,
    /// as its tail does (`0x4d83f4`), and the stock window re-reads `GetMoney()` on it.
    pub(crate) re_derive: u32,
    /// Per service, whether its record's group key is the known group's -1: written by the list
    /// for a row it gets known at a mount trainer (`0x4d77e8`) and by each pass for a row it leaves
    /// known (`0x4d82b2`-`0x4d82c1`), and never written back until the next list.
    pub(crate) known_key: Vec<bool>,
    /// The item records the icon and description getters' calls missed, one per call: the item
    /// cache's callbacks `0x4d9140` and `0x4d9db0`, which outlive the window and run `0x4d7d40`
    /// when their record lands (`0x4d914a`, `0x4d9dba`), on whatever trainer is open then.
    pub(crate) item_waits: Vec<u32>,
}

impl TrainerOpen {
    /// Open (or replace) the window with a trainer's freshly-listed services.
    pub(crate) fn open(
        &mut self,
        trainer: u64,
        trainer_type: u32,
        services: Vec<TrainerSpell>,
        greeting: String,
    ) {
        self.trainer = Some(trainer);
        self.trainer_type = trainer_type;
        self.known_key = services
            .iter()
            .map(|w| trainer_type == TRAINER_TYPE_MOUNT && w.state == trainer_spell_state::GRAY)
            .collect();
        self.services = services;
        self.greeting = greeting;
        // A fresh list carries the server's own states, so nothing is pending against it.
        self.fresh_list = true;
        self.re_derive = 0;
    }

    /// One of the twelve triggers fired: one more pass on the next feed, if a window is open.
    pub(crate) fn trigger_re_derive(&mut self) {
        if self.trainer.is_some() {
            self.re_derive += 1;
        }
    }

    /// Whether a pass is owed.
    #[cfg(test)]
    pub(crate) fn re_derive_owed(&self) -> bool {
        self.re_derive > 0
    }

    /// A client-side close; a re-open re-lists.
    pub(crate) fn clear(&mut self) {
        self.trainer = None;
        self.services.clear();
        self.trainer_type = 0;
        self.greeting.clear();
        self.known_key.clear();
        self.fresh_list = false;
        self.re_derive = 0;
    }

    /// Disconnect: drop the open window, and the item callbacks with the asks they wait on.
    pub(crate) fn clear_session(&mut self) {
        self.clear();
        self.item_waits.clear();
    }

    /// Spend the waits whose item record answered: each one that brought a record is a trigger of
    /// its own pass; a refusal runs its callback with status 0, which does nothing (`0x4d9146`,
    /// `0x4d9db6`).
    fn item_answers(&mut self, items: &Items) {
        let mut landed = 0;
        self.item_waits.retain(|&entry| {
            if !items.template_answered(entry) {
                return true;
            }
            if items.template(entry, 0, Ask::Read).is_some() {
                landed += 1;
            }
            false
        });
        for _ in 0..landed {
            self.trigger_re_derive();
        }
    }
}

/// `SMSG_TRAINER_BUY_FAILED` codes, queued for the feed to log.
#[derive(Resource, Default)]
pub(crate) struct TrainerErrors(pub Vec<u32>);

/// The spells the open trainer's services show as tooltips, which the spell-tooltip feed pushes:
/// a pet-learn service's view is built against the pet (the builder's unit selector, `0x533a7f`).
#[derive(Resource, Default, PartialEq)]
pub(crate) struct TrainerTooltipSubjects {
    pub(crate) player: Vec<u32>,
    pub(crate) pet: Vec<u32>,
}

/// The trainer feed, which the spell-tooltip feed runs after.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct TrainerFeed;

mod net;
mod reeval;

pub(crate) struct UiTrainerPlugin;

impl Plugin for UiTrainerPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<TrainerOpen>()
            .init_resource::<TrainerErrors>()
            .init_resource::<TrainerTooltipSubjects>()
            .add_latched_window::<TrainerOpen>()
            // `0x490ad5` → `0x4d7400` → `0x4d7440(0)`, the core.
            .add_sweep_leg(SweepLeg::Trainer, close_core)
            .add_systems(
                Update,
                (
                    // The unit feeds also write the pet bar the re-evaluator reads; either order
                    // reads the same pet spellbook, but the schedule check wants one declared.
                    feed_trainer
                        .in_set(UiFeed)
                        .in_set(TrainerFeed)
                        .in_set(crate::ui_session::NpcWindowFeed)
                        .after(crate::ui_unit::UnitFeed),
                ),
            );
    }
}

/// The log line for a `SMSG_TRAINER_BUY_FAILED` code. The 1.12 client shows the player nothing:
/// its handler (`0x5e5f10`, registered at `0x5e3291`) prints these strings (`0x8604bc`,
/// `0x860494`, `0x860464`) to the developer console (`0x63cd00`) from its arms (`0x5e5f95`,
/// `0x5e5fb8`, `0x5e5fdc`), and `BuyTrainerService` (`0x4da210`) tests nothing before sending.
fn train_error_log_line(code: u32) -> &'static str {
    use benilla_protocol::messages::train_fail;
    match code {
        train_fail::NOT_ENOUGH_MONEY => "Not enough money for trainer service",
        train_fail::NOT_ENOUGH_SKILL => "Not enough skill points for trainer service",
        _ => "Trainer service unavailable",
    }
}

/// The spellbooks `GetTrainerServiceAbilityReq` asks (`0x4d9850`-`0x4d9882`): the player's, and for
/// a `LEARN_PET_SPELL` service the unit `UNIT_FIELD_SUMMON` names, `None` when no such object is
/// streamed. `IsSpellKnown` reads a unit's book only as the pet bar's unit (`0x60c77a`-`0x60c790`),
/// so any other summon's is empty.
pub(super) struct ReqBooks<'a> {
    pub(super) player: &'a BTreeSet<u32>,
    pub(super) pet: Option<&'a BTreeSet<u32>>,
    /// The player's race and class, which index the taught spell's line (`0x60c920` →
    /// `0x6de040`).
    pub(super) race_class: (u8, u8),
}

/// Resolve one wire [`TrainerSpell`] into the Lua [`TrainerService`]; `books` colour each
/// prerequisite ability.
fn resolve_service(
    wire: &TrainerSpell,
    trainer_type: u32,
    spells: &SpellCatalog,
    skill_lines: Option<&SkillLineCatalog>,
    books: &ReqBooks,
    icons: Option<&ItemDisplays>,
    items: &Items,
    ask: Ask<'_>,
    text: &ServiceText,
    // The VM's own `GlobalStrings.lua`, for the group header labels.
    get: &dyn Fn(&str) -> Option<String>,
) -> TrainerService {
    // `taught` feeds only the group key (`0x4d7c60`); the name and subtext are the wire wrapper's
    // own columns (`0x4d8aa0`, `0x4d8b50`), which differ from the taught spell's on every
    // profession-learn row. The wrapper stays the buy id.
    let taught = spells.learned_spell(wire.spell).unwrap_or(wire.spell);
    let display = spells.get(wire.spell);
    let cat = category(wire.state);
    // The wire has no per-gate bit, so the skill gate follows the service's category; the 1.12
    // client compares the player's skill value itself, which is not built here.
    let skill_met = cat != TrainerServiceCategory::Unavailable;
    let skill_req = (wire.req_skill != 0).then(|| TrainerSkillReq {
        name: skill_lines
            .and_then(|c| c.line(wire.req_skill))
            .map(|l| l.name.clone())
            .unwrap_or_else(|| format!("Skill {}", wire.req_skill)),
        rank: wire.req_skill_value,
        met: skill_met,
    });
    // `GetTrainerServiceAbilityReq`'s flag (`0x4d96e0`), apart from the category: it starts at 1
    // (`0x4d9761`) and clears only when the requirement's record and the service spell's resolve
    // (`0x4d97c8`, `0x4d9842`) and the book holds neither the ability (`0x60c740`) nor a later
    // rank of it (`0x60c8d0`). A pet-learn service asks the pet's book, and no pet keeps the 1.
    let book = match display {
        None => None,
        Some(d) if d.effects.contains(&SPELL_EFFECT_LEARN_PET_SPELL) => books.pet,
        Some(_) => Some(books.player),
    };
    // All three slots, in place: the getter reads slot `i - 1` (`0x4d97a5`).
    let ability_reqs = wire
        .req_spells
        .iter()
        .map(|&s| {
            let req = spells.get(s);
            let known = |book: &BTreeSet<u32>| {
                book.contains(&s) || skill_lines.is_some_and(|c| c.higher_rank_known(s, book))
            };
            TrainerAbilityReq {
                spell_id: s as i32,
                name: req.map(|d| d.ranked_name()),
                met: req.is_none() || book.is_none_or(known),
            }
        })
        .collect();
    let (is_trade_skill, learn_spell) = service_kind(wire.spell, spells);
    // The wrapper is never in `SkillLineAbility`, so the skill-line arm keys on the taught spell;
    // a service it cannot place drops, as in the client's builder, and is logged.
    let (group_key, group_name) = service_group(
        wire.spell,
        taught,
        trainer_type,
        cat,
        spells,
        skill_lines.map(|c| (c, books.race_class)),
        get,
    );
    if group_key == 0 && skill_lines.is_some() {
        debug!(
            "ui_trainer: trainer spell {} (teaches {taught}) has no skill line — dropped from the tree",
            wire.spell
        );
    }
    let (mut icon_wait, mut description_wait) = (None, None);
    // Over the wire spell, like the name and subtext ([`service_icon`]).
    let texture = service_icon_waiting(
        wire.spell,
        trainer_type,
        spells,
        icons,
        items,
        ask,
        &mut icon_wait,
    );
    let description = service_description_waiting(
        wire,
        trainer_type,
        spells,
        items,
        ask,
        text,
        &mut description_wait,
    );
    TrainerService {
        spell_id: wire.spell,
        name: display.map(|d| d.name.clone()),
        // `0x4d8b50` answers the record's NameSubtext, "" for a rankless spell (`0x4d8b84`), and
        // NULL without a Spell record (`0x4d8b58`), which `0x6f3890` pushes as nil.
        subtext: display.map(|d| d.rank.clone().unwrap_or_default()),
        texture,
        icon_wait,
        description,
        description_wait,
        cost: wire.cost,
        prof_first_rank: wire.is_primary_prof_first_rank,
        category: cat,
        level_req: u32::from(wire.req_level),
        skill_req,
        ability_reqs,
        is_trade_skill,
        learn_spell,
        group_key,
        group_name,
        tooltip: service_tooltip(wire.spell, spells),
        skill_step: display.is_some_and(|d| d.effects.contains(&SPELL_EFFECT_SKILL_STEP)),
        step_increase: step_increase(wire.spell, spells, skill_lines, get),
        line_name: skill_lines
            .and_then(|c| c.line(group_key))
            .map(|l| l.name.clone()),
    }
}

/// `GetTrainerServiceStepIncrease`'s text (`0x4da030`) for a wire spell's first skill-step effect:
/// `INCREASE_POTENTIAL` over the effect's skill line name and five times its step (the previous
/// step, record `+0x20`, is always 0). Nothing when there is no such effect, the step is 0 or the
/// line has no row; the binding adds the used-row refusal.
fn step_increase(
    wire_spell: u32,
    spells: &SpellCatalog,
    skill_lines: Option<&SkillLineCatalog>,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    let (skill, step) = spells
        .learn_effects(wire_spell)
        .iter()
        .find_map(|e| match *e {
            LearnEffect::SkillStep { skill, step } => Some((skill, step)),
            _ => None,
        })?;
    if step == 0 {
        return None;
    }
    let line = skill_lines?.line(skill)?;
    let template = get("INCREASE_POTENTIAL")?;
    Some(benilla_ui::strings::fill(
        &template,
        &[
            benilla_ui::strings::Arg::S(&line.name),
            benilla_ui::strings::Arg::D(5 * i64::from(step)),
        ],
    ))
}

/// Build the Lua snapshot from [`TrainerOpen`] and the spell and skill catalogs.
fn snapshot(
    open: &TrainerOpen,
    spells: &SpellCatalog,
    skill_lines: Option<&SkillLineCatalog>,
    books: &ReqBooks,
    icons: Option<&ItemDisplays>,
    items: &Items,
    ask: Ask<'_>,
    text: &ServiceText,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<TrainerState> {
    let guid = open.trainer?;
    Some(TrainerState {
        guid,
        greeting: open.greeting.clone(),
        trainer_type: open.trainer_type,
        services: open
            .services
            .iter()
            .zip(open.known_key.iter().chain(std::iter::repeat(&false)))
            .map(|(w, &known_key)| {
                let mut service = resolve_service(
                    w,
                    open.trainer_type,
                    spells,
                    skill_lines,
                    books,
                    icons,
                    items,
                    ask,
                    text,
                    get,
                );
                // The record keeps the -1 a list or a pass wrote, whatever its state now.
                if known_key && service.group_key != TRAINER_GROUP_KNOWN {
                    service.group_key = TRAINER_GROUP_KNOWN;
                    service.group_name = get("KNOWN_TALENTS_HEADER").unwrap_or_default();
                    service.line_name = None;
                }
                service
            })
            .collect(),
        // The engine builds the tree in `set_trainer` from each service's group key.
        groups: Vec::new(),
    })
}

/// The re-evaluator's inputs and its descriptor triggers: the player's descriptor and its field
/// edges (money, level, a skill slot), the pet bar, and the stores that resolve the pet.
#[derive(SystemParam)]
pub(crate) struct ReEvalInputs<'w, 's> {
    self_player: Query<'w, 's, (Entity, &'static ObjectStore), With<SelfPlayer>>,
    stores: Query<'w, 's, &'static ObjectStore>,
    index: Res<'w, GuidIndex>,
    pet_bar: Res<'w, crate::ui_pet::PetBar>,
    edges: MessageReader<'w, 's, FieldChanged>,
}

/// The resources a service description's `$`-tokens read beside the caster's descriptor: the
/// spell-modifier tables and the bind point `$z` names, the same two reads the item and spell
/// tooltip feeds make.
#[derive(SystemParam)]
pub(crate) struct DescriptionReads<'w> {
    spell_mods: Res<'w, crate::spell::SpellModifiers>,
    home_bind: Option<Res<'w, crate::net::HomeBind>>,
    area_names: Option<Res<'w, crate::ui_quest_log::QuestHeaderNamesRes>>,
}

/// Push the current trainer into the VM and fire its events on a change; the close is the
/// core's ([`close_core`]).
#[allow(clippy::too_many_arguments)] // one Bevy system's full input set
pub(crate) fn feed_trainer(
    // The window it writes the re-derived states to, as `0x4d7d40` overwrites its own records.
    mut vm: crate::script_calls::VmWith<(
        ReEvalInputs<'static, 'static>,
        ResMut<'static, TrainerOpen>,
    )>,
    actions: Res<PlayerActions>,
    spells: Option<Res<Spells>>,
    skill_lines: Option<Res<SkillLines>>,
    desc: DescriptionReads,
    // A tradeskill row shows its created item's icon: the template cache and `ItemDisplayInfo.dbc`.
    icons: Option<Res<ItemDisplays>>,
    items: Res<Items>,
    mut errors: ResMut<TrainerErrors>,
    commands: Res<NetCommands>,
    names: Res<NameCache>,
    mut tooltip_subjects: ResMut<TrainerTooltipSubjects>,
    mut last: crate::ui_script::WindowMemo<Option<TrainerState>>,
    mut closes: CoreCloses,
) {
    let mut extra_passes = 0;
    vm.feed(|script, (mut re_eval, mut open), fires| {
        let Some(script) = script else {
            return;
        };
        let last = last.get(script);
        if closes.since_last(script, NpcWindow::Trainer) {
            *last = None;
        }
        // Refusals go to the log only, as the reference prints them only to its console.
        for code in errors.0.drain(..) {
            info!("ui_trainer: {} (code {code})", train_error_log_line(code));
        }
        // Nothing to resolve names or icons from until Spell.dbc loads.
        let Some(spells) = spells.as_deref() else {
            return;
        };
        // Without the skill-line catalog every service would group to 0 and drop.
        let Some(skill_lines) = skill_lines.as_deref() else {
            return;
        };
        // The getters' item callbacks, kept by their calls; a record that answered one is a trigger.
        open.item_waits.extend(script.take_trainer_item_waits());
        open.item_answers(&items);
        // The per-list reset goes in before the snapshot, so `TRAINER_SHOW` finds it; the stock
        // window re-applies its saved filter only on its own `ADDON_LOADED`.
        let listed = open.fresh_list;
        if open.fresh_list {
            script.reset_trainer_list_state(open.trainer_type);
            open.fresh_list = false;
        }
        // The player's money, level and skill triggers, read every frame so the reader never backs
        // up.
        let self_player = re_eval.self_player.iter().next();
        for edge in re_eval.edges.read() {
            let Some((me, _)) = self_player else {
                continue;
            };
            if edge.entity != me {
                continue;
            }
            // The reference watches each slot's value, max and permanent bonus (`0x5de180`,
            // `0x5de450`): the second dword always, the third only when its high half moved.
            let skill_edge = edge
                .index
                .checked_sub(FIELD_PLAYER_SKILL_INFO_1_1)
                .filter(|&r| r < 3 * u16::from(PLAYER_SKILL_SLOTS))
                .is_some_and(|r| match r % 3 {
                    1 => true,
                    2 => (edge.old ^ edge.new) & 0xFFFF_0000 != 0,
                    _ => false,
                });
            let player_field = edge.kind == benilla_protocol::messages::ObjectType::Player
                && (edge.index == FIELD_PLAYER_FIELD_COINAGE || skill_edge);
            if player_field || edge.unit_field(FIELD_UNIT_LEVEL) {
                open.trigger_re_derive();
            }
        }
        // `0x4d7d40` over every row, then `TRAINER_UPDATE` unconditionally, as its tail fires it,
        // once per trigger.
        let passes = if open.trainer.is_some() {
            std::mem::take(&mut open.re_derive)
        } else {
            0
        };
        let forced = passes > 0;
        // The stock window calls `GetTrainerServiceIcon` and `GetTrainerServiceDescription`, which
        // look the product up with a callback (`0x4d9096`, `0x4d9d07`), at `TRAINER_SHOW` and
        // `TRAINER_UPDATE`: a list, a re-derive, or a window this VM has not shown.
        let ask = Ask::at(last.is_none() || listed || forced, &commands);
        if forced {
            let player = reeval::PlayerView {
                known: actions.spells.clone(),
                level: self_player
                    .and_then(|(_, store)| store.0.unit_level())
                    .unwrap_or(0),
                skills: self_player
                    .map(|(_, store)| {
                        (0..PLAYER_SKILL_SLOTS)
                            .filter_map(|i| store.0.player_skill(i))
                            .filter(|s| s.skill_id != 0)
                            .map(|s| reeval::SkillSlot {
                                skill_id: u32::from(s.skill_id),
                                step: u32::from(s.step),
                                // The permanent bonus counts only on a non-zero value (`0x4d8087`).
                                value_plus_perm: if s.value == 0 {
                                    0
                                } else {
                                    i32::from(s.value) + i32::from(s.perm_bonus)
                                },
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                pet: (re_eval.pet_bar.spells.pet_guid != 0)
                    .then(|| re_eval.index.0.get(&re_eval.pet_bar.spells.pet_guid))
                    .flatten()
                    .and_then(|&e| re_eval.stores.get(e).ok())
                    .map(|store| reeval::PetView {
                        level: store.0.unit_level().unwrap_or(0),
                        known: re_eval
                            .pet_bar
                            .spells
                            .spells
                            .iter()
                            .filter(|e| e.is_spell())
                            .map(|e| e.action())
                            .collect(),
                    }),
            };
            let TrainerOpen {
                services,
                trainer_type,
                known_key,
                ..
            } = &mut *open;
            reeval::re_derive_all(
                services,
                *trainer_type,
                &spells.catalog,
                &skill_lines.catalog,
                &player,
            );
            // `0x4d82b2`-`0x4d82c1`: a row the pass leaves known takes the -1 key at type 1.
            if *trainer_type == TRAINER_TYPE_MOUNT {
                for (key, wire) in known_key.iter_mut().zip(services.iter()) {
                    *key |= wire.state == trainer_spell_state::GRAY;
                }
            }
        }
        // The spell tooltip's description reads (`0x52f717`), and `0x6e3159`'s charm-else-summon
        // for a `LEARN_PET_SPELL` row.
        let store = self_player.map(|(_, store)| store);
        let pet = store
            .and_then(|s| s.0.unit_pet_guid())
            .and_then(|guid| re_eval.index.0.get(&guid).copied())
            .and_then(|entity| re_eval.stores.get(entity).ok());
        let fresh = {
            let gender = store.and_then(|s| s.0.unit_gender()).unwrap_or(0);
            let home_area: Option<&str> = desc
                .home_bind
                .as_deref()
                .and_then(|b| b.0)
                .and_then(|id| desc.area_names.as_deref()?.0.resolve(id as i32));
            let skills = crate::spell::PlayerSkills {
                lines: Some(&skill_lines.catalog),
                spells: Some(&spells.catalog),
                items: Some(&items),
            };
            let player_level = |d: &benilla_formats::SpellDisplay| {
                crate::spell::player_spell_level(store, skills, d, &mut crate::spell::Unrecorded)
            };
            let pet_skill_level = |d: &benilla_formats::SpellDisplay| {
                d.skill_level(crate::spell::pet_skill_value(pet.map(|p| &p.0)))
            };
            // `$c` and `$p` (`0x6e31b0`, `0x6e3300`) at the same caster's level and fields.
            let mods = &*desc.spell_mods;
            let player_cost = |d: &benilla_formats::SpellDisplay| {
                store.map_or(-1, |s| {
                    crate::spell::usable::power_cost(
                        d,
                        s,
                        skills,
                        mods,
                        &mut crate::spell::Unrecorded,
                    )
                })
            };
            let player_per_second = |d: &benilla_formats::SpellDisplay| {
                let level = crate::spell::SpellLevel::Player { me: store, skills };
                crate::spell::usable::per_second_cost(d, level, &mut crate::spell::Unrecorded)
            };
            let pet_level = |d: &benilla_formats::SpellDisplay| {
                crate::spell::SpellLevel::Known(pet_skill_level(d))
            };
            let pet_cost = |d: &benilla_formats::SpellDisplay| {
                pet.map_or(-1, |p| {
                    let level = pet_level(d);
                    crate::spell::usable::power_cost_at(
                        d,
                        &p.0,
                        level,
                        mods,
                        &mut crate::spell::Unrecorded,
                    )
                })
            };
            let pet_per_second = |d: &benilla_formats::SpellDisplay| {
                crate::spell::usable::per_second_cost(
                    d,
                    pet_level(d),
                    &mut crate::spell::Unrecorded,
                )
            };
            let get = |key: &str| {
                script
                    .lua()
                    .globals()
                    .get::<String>(key)
                    .ok()
                    .filter(|t| !t.is_empty())
            };
            let ctx = TokenContext {
                durations: &spells.durations,
                radii: &spells.radii,
                ranges: Some(&spells.ranges),
                level: &player_level,
                lookup: &|id| spells.catalog.get(id),
                mods: Some(mods),
                power_cost: &player_cost,
                power_per_second: &player_per_second,
                unmodified_points: false,
                gender: &|| gender,
                home_area: &|| home_area,
                global: &get,
                printf: &crate::ui_script::token_printf,
            };
            let pet_ctx = TokenContext {
                level: &pet_skill_level,
                power_cost: &pet_cost,
                power_per_second: &pet_per_second,
                ..ctx
            };
            // `UNIT_FIELD_SUMMON`'s unit (`0x4d9860`-`0x4d987d`), its book read only as the pet
            // bar's (`0x60c77a`).
            let pet_book: Option<BTreeSet<u32>> = store
                .and_then(|s| s.0.unit_summon())
                .filter(|guid| re_eval.index.0.contains_key(guid))
                .map(|guid| {
                    let bar = &re_eval.pet_bar.spells;
                    if guid != bar.pet_guid {
                        return BTreeSet::new();
                    }
                    bar.spells
                        .iter()
                        .filter(|e| e.is_spell())
                        .map(|e| e.action())
                        .collect()
                });
            snapshot(
                &open,
                &spells.catalog,
                Some(&skill_lines.catalog),
                &ReqBooks {
                    player: &actions.spells,
                    pet: pet_book.as_ref(),
                    race_class: store.map_or((0, 0), |s| s.race_class()),
                },
                icons.as_deref(),
                &items,
                ask,
                &ServiceText {
                    player: &ctx,
                    pet: &pet_ctx,
                },
                &get,
            )
        };
        // A spell subject's view must be in the store before the detail icon is hovered.
        let mut fresh_tooltip_subjects = TrainerTooltipSubjects::default();
        for service in fresh.iter().flat_map(|state| &state.services) {
            if let TrainerTooltip::Spell {
                spell_id,
                alt_caster,
            } = service.tooltip
            {
                let subjects = if alt_caster {
                    &mut fresh_tooltip_subjects.pet
                } else {
                    &mut fresh_tooltip_subjects.player
                };
                subjects.push(spell_id);
            }
        }
        for subjects in [
            &mut fresh_tooltip_subjects.player,
            &mut fresh_tooltip_subjects.pet,
        ] {
            subjects.sort_unstable();
            subjects.dedup();
        }
        if *tooltip_subjects != fresh_tooltip_subjects {
            *tooltip_subjects = fresh_tooltip_subjects;
        }
        // The stock window titles with `UnitName("npc")` (`Blizzard_TrainerUI.lua:90`), which
        // looks the name up with a callback (`0x5171b8`) at its events; its landing fires nothing.
        if let Some(g) = open.trainer {
            let store = re_eval
                .index
                .0
                .get(&g)
                .and_then(|&e| re_eval.stores.get(e).ok());
            let _ = names.resolve_unit(g, store, ask);
        }
        // A list lays the rows out anew, so it pushes even when it matches the last.
        if fresh == *last && !forced && !listed {
            return;
        }
        // The rows sort only where the reference finalizes them: a list (`0x4d7b3b`) and the
        // state pass `0x4d7d40`, which an item record a getter missed runs too (`0x4d914a`,
        // `0x4d9dba`); a name, an icon or a record no getter asked for leaves their order (and
        // their ties).
        let shows = fresh.is_some() && (listed || last.is_none());
        if shows || forced {
            script.set_trainer(fresh.clone());
        } else {
            script.refresh_trainer(fresh.clone());
        }
        // Each event has its own producers, neither carrying an argument: `TRAINER_SHOW` the list
        // builder's (`0x4d7b73`), every list; `TRAINER_UPDATE` the state pass's (`0x4d83f9`). A
        // repaint with neither fires nothing; the close is the core's.
        if shows {
            fires.post("TRAINER_SHOW", vec![]);
            extra_passes = passes;
        } else if forced && fresh.is_some() {
            fires.post("TRAINER_UPDATE", vec![]);
            extra_passes = passes - 1;
        }
        *last = fresh;
    });
    // The passes past the first, each its own sort from the order the last left and its own
    // `TRAINER_UPDATE`, whose handlers run before the next pass sorts.
    if extra_passes > 0 {
        vm.run(|script| {
            if let Some(script) = script {
                for _ in 0..extra_passes {
                    script.rerun_trainer_finalize();
                    script.fire_event("TRAINER_UPDATE", vec![]);
                }
            }
        });
    }
}

/// The trainer's cell `[0xb73a00]`; `CloseTrainer` (`0x4d8d50`) is the core with a zero guid, run
/// inside its call ([`crate::script_calls::ScriptCallHost`]).
impl LatchedWindow for TrainerOpen {
    const WINDOW: NpcWindow = NpcWindow::Trainer;

    fn cell(&self) -> Option<u64> {
        self.trainer
    }

    fn dispatched(world: &mut World) {
        close_core(world);
    }
}

/// The close core, `0x4d7440` with a zero guid, as `0x493310` and the sweep enter it: nothing with
/// no trainer (`0x4d7459`, a zero guid against a zero cell), else `TRAINER_CLOSED` first
/// (`0x4d7468`), then the latch release on the cell as it stands after the fire (`0x4d7474`) and
/// the cell cleared (`0x4d747c`). The fire comes before the clear, so `OnHide`'s `CloseTrainer()`
/// re-enters with the cell set.
pub(crate) fn close_core(world: &mut World) {
    if world.resource::<TrainerOpen>().trainer.is_none() {
        return;
    }
    crate::ui_session::fire(world, |script| {
        script.fire_event("TRAINER_CLOSED", vec![]);
    });
    if let Some(trainer) = world.resource::<TrainerOpen>().trainer {
        crate::ui_session::release(world, trainer);
    }
    if world.resource::<TrainerOpen>().trainer.is_some() {
        world.resource_mut::<TrainerOpen>().clear();
        crate::ui_session::note_closed(world, NpcWindow::Trainer);
        crate::ui_session::with_script(world, |script| script.set_trainer(None));
    }
}

/// `SMSG_TRAINER_LIST`'s open, `0x4d7440(trainer)`: the open trainer's own list re-lists in place
/// (`0x4d7459`); any other registers on the latch (`0x4d74ce`), then the cell (`0x4d74d5`).
pub(crate) fn open_trainer(
    world: &mut World,
    trainer: u64,
    trainer_type: u32,
    services: Vec<TrainerSpell>,
    greeting: String,
) {
    if world.resource::<TrainerOpen>().trainer != Some(trainer) {
        crate::ui_session::register(world, trainer);
    }
    world
        .resource_mut::<TrainerOpen>()
        .open(trainer, trainer_type, services, greeting);
}

/// The trainer verbs' engine half, which the call host applies inside each call
/// ([`crate::script_calls::ScriptCallHost`]).
#[derive(SystemParam)]
pub(crate) struct TrainerSteps<'w, 's> {
    /// Absent in a world with no trainer window, which closes nothing.
    open: Option<ResMut<'w, TrainerOpen>>,
    net: Res<'w, NetCommands>,
    me: Query<'w, 's, &'static Guid, With<SelfPlayer>>,
}

impl TrainerSteps<'_, '_> {
    /// `BuyTrainerService`'s send, with the trainer's cell as the VM's open window holds it
    /// (`0x4d8a11`).
    pub(crate) fn buy(&mut self, trainer: u64, spell_id: u32) {
        debug!("ui_trainer: buy service (trainer {trainer:#x}, spell {spell_id})");
        let _ = self
            .net
            .0
            .send(ClientCommand::TrainerBuySpell { trainer, spell_id });
    }

    /// The active player's guid, which `OpenTrainer` needs (`0x4d8d24`).
    pub(crate) fn own_guid(&self) -> Option<u64> {
        self.me.single().ok().map(|&Guid(g)| g)
    }

    /// The rest of `TalkToTrainer` on the player's own guid (`0x5df680`) after its close
    /// (`0x5df6bb`): `CMSG_TRAINER_LIST` (`0x5df6fc`), which the server answers with nothing for
    /// a player.
    pub(crate) fn ask_own_list(&mut self, trainer: u64) {
        debug!("ui_trainer: OpenTrainer asks for the player's own list");
        let _ = self.net.0.send(ClientCommand::TrainerList { trainer });
    }

    /// The trainer's cell `[0xb73a00]`.
    pub(crate) fn trainer(&self) -> Option<u64> {
        self.open.as_deref().and_then(|o| o.trainer)
    }

    /// The close core's clear of the cell (`0x4d747c`), and the VM's window with it: whether a
    /// trainer was stored.
    pub(crate) fn clear_trainer(&mut self, script: &mut UiScript) -> bool {
        let Some(open) = self.open.as_deref_mut().filter(|o| o.trainer.is_some()) else {
            return false;
        };
        open.clear();
        script.set_trainer(None);
        true
    }
}

mod law;
use law::{
    category, service_description_waiting, service_group, service_icon_waiting, service_kind,
    service_tooltip, ServiceText,
};
#[cfg(test)]
use law::{service_description, service_icon};

#[cfg(test)]
mod call_tests;
#[cfg(test)]
mod tests;
