//! The trainer window: [`TrainerOpen`] from `SMSG_TRAINER_LIST`, its events on the trainer lane
//! at their handlers ([`lane`]), where each list's and each state pass's firing pushes the
//! resolved [`TrainerService`]s and sorts the rows; the getters' per-call reads through the call
//! host ([`live`]); and [`TrainerSteps`] sending the buys and running `CloseTrainer`'s close core
//! inside their calls. A bought row repaints through the state pass ([`reeval`]), never through
//! `SMSG_TRAINER_BUY_SUCCEEDED`, and a refusal goes only to the log.

use std::collections::BTreeSet;

use benilla_formats::{
    LearnEffect, SkillLineCatalog, SpellCatalog, SPELL_EFFECT_LEARN_PET_SPELL,
    SPELL_EFFECT_LEARN_SPELL, SPELL_EFFECT_SKILL_STEP,
};
use benilla_protocol::messages::{trainer_spell_state, TrainerSpell};
use bevy::prelude::*;

use benilla_ui::script::{
    TrainerAbilityReq, TrainerService, TrainerServiceLive, TrainerSkillReq, TrainerState, UiScript,
    TRAINER_GROUP_KNOWN,
};

use crate::entities::ItemDisplays;
use crate::items::Items;
use benilla_protocol::messages::PLAYER_SKILL_SLOTS;
use bevy::ecs::system::SystemParam;

use crate::net::{ClientCommand, Guid, GuidIndex, NetCommands, ObjectStore, SelfPlayer};
use crate::query_cache::Ask;
use crate::ui_action::{PlayerActions, Spells};
use crate::ui_script::{SweepApp, SweepLeg};
use crate::ui_session::{LatchApp, LatchedWindow, NpcWindow};
use crate::ui_spellbook::SkillLines;

pub(crate) use lane::{TrainerEvent, TrainerLane};
pub(crate) use live::TrainerLive;

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

/// `TalkToTrainer` (`0x5df680`) on `trainer`: the asked guid stored (`0x5df69f`, `0x5df6a8`), the
/// window closed through the close core (`0x5df6bb` → `0x4d7440` with a zero guid), then
/// `CMSG_TRAINER_LIST` for it (`0x5df6fc`). Its callers are `SetPetGuid`'s refresh (`0x4bc8b2`),
/// `OpenTrainer` (`0x4d8d3c`) and the NPC click (`0x5f0385`).
pub(crate) fn talk_to_trainer(world: &mut World, trainer: u64) {
    if let Some(mut open) = world.get_resource_mut::<TrainerOpen>() {
        open.asked = trainer;
    }
    close_core(world);
    if let Some(net) = world.get_resource::<NetCommands>() {
        let _ = net.0.send(ClientCommand::TrainerList { trainer });
    }
}

/// The open trainer as `SMSG_TRAINER_LIST` delivered it, and its state passes rewrote it.
#[derive(Resource, Default)]
pub(crate) struct TrainerOpen {
    pub(crate) trainer: Option<u64>,
    /// In wire order, which is the 1-based display order; each pass rewrites the states.
    pub(crate) services: Vec<TrainerSpell>,
    pub(crate) trainer_type: u32,
    pub(crate) greeting: String,
    /// Per service, whether its record's group key is the known group's -1: written by the list
    /// for a row it gets known at a mount trainer (`0x4d77e8`) and by each pass for a row it leaves
    /// known (`0x4d82b2`-`0x4d82c1`), and never written back until the next list.
    pub(crate) known_key: Vec<bool>,
    /// The trainer `TalkToTrainer` last asked for its list (`0xc4d788`), which a list's open tests
    /// (`0x5dcc85`): written by every ask, never cleared, 0 before the first.
    pub(crate) asked: u64,
    /// One step per list, which the VM's rows carry: a live read answers only for its own list.
    pub(crate) list: u64,
    /// The trainer the records were listed by, which a close leaves: the records stay until the
    /// next list (the close zeroes only the cell and the counts, `0x4d747c`-`0x4d7486`).
    pub(crate) listed: Option<u64>,
    /// The cascade's reset, owed from its item's push ([`TrainerOpen::settle_world_reset`]).
    pub(crate) world_reset: crate::ui_world_enter::OwedReset,
}

impl TrainerOpen {
    /// Open (or replace) the window with a trainer's freshly-listed services, past an owed reset.
    pub(crate) fn open(
        &mut self,
        trainer: u64,
        trainer_type: u32,
        services: Vec<TrainerSpell>,
        greeting: String,
    ) {
        self.settle_world_reset();
        self.trainer = Some(trainer);
        self.listed = Some(trainer);
        self.trainer_type = trainer_type;
        self.known_key = services
            .iter()
            .map(|w| trainer_type == TRAINER_TYPE_MOUNT && w.state == trainer_spell_state::GRAY)
            .collect();
        self.services = services;
        self.greeting = greeting;
        self.list = self.list.wrapping_add(1);
    }

    /// The close's clear of the cell (`0x4d747c`); a re-open re-lists.
    pub(crate) fn clear(&mut self) {
        self.trainer = None;
    }

    /// The session end: no window and no records, the ask and the list count kept.
    pub(crate) fn clear_session(&mut self) {
        *self = Self {
            asked: self.asked,
            list: self.list,
            ..Self::default()
        };
    }
}

/// One of the twelve callers of the state pass `0x4d7d40` ran: with a window open, its pass and its
/// `TRAINER_UPDATE` on the lane at the caller's place.
pub(crate) fn pass_owed(open: Option<&TrainerOpen>, lane: Option<&mut TrainerLane>) {
    if let (Some(lane), true) = (lane, open.is_some_and(|o| o.trainer.is_some())) {
        lane.push(TrainerEvent::Update);
    }
}

/// The state pass's callers inside the object packets' handlers: the descriptor watches
/// ([`crate::field_watch::FieldNotice::TrainerPass`]) and the getters' item callbacks.
#[derive(SystemParam)]
pub(crate) struct TrainerPasses<'w> {
    open: Option<ResMut<'w, TrainerOpen>>,
    lane: Option<ResMut<'w, TrainerLane>>,
}

impl TrainerPasses<'_> {
    /// One caller's pass, at its place ([`pass_owed`]).
    pub(crate) fn owed(&mut self) {
        pass_owed(self.open.as_deref(), self.lane.as_deref_mut());
    }

    /// Our create's trainer reset, owed from its packet ([`TrainerOpen::settle_world_reset`]).
    pub(crate) fn owe_world_reset(&mut self) {
        if let Some(open) = self.open.as_mut() {
            open.world_reset.owe();
        }
    }
}

/// A trainer getter's product lookup with its callback, inside its call
/// ([`benilla_ui::script::ItemCallback::Trainer`]): the item cache asks a miss, a refused entry
/// again, and a miss keeps one callback per call (`0x4d907e`, `0x4d9cef`), whose landing runs the
/// state pass ([`crate::items::RecordCallback::Trainer`]).
pub(crate) fn ask_item_record(items: &Items, commands: &NetCommands, entry: u32, owner: u64) {
    if items.template(entry, owner, Ask::Event(commands)).is_none() {
        items.wait(entry, crate::items::RecordCallback::Trainer);
    }
}

mod lane;
mod live;
mod net;
mod reeval;

pub(crate) struct UiTrainerPlugin;

impl Plugin for UiTrainerPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        lane::add_trainer_lane(app);
        app.init_resource::<TrainerOpen>()
            .add_latched_window::<TrainerOpen>()
            // `0x490ad5` → `0x4d7400` → `0x4d7440(0)`, the core.
            .add_sweep_leg(SweepLeg::Trainer, close_core);
    }
}

/// The log line for a `SMSG_TRAINER_BUY_FAILED` code. The 1.12 client shows the player nothing:
/// its handler (`0x5e5f10`, registered at `0x5e3291`) prints these strings (`0x8604bc`,
/// `0x860494`, `0x860464`) to the developer console (`0x63cd00`) from its arms (`0x5e5f95`,
/// `0x5e5fb8`, `0x5e5fdc`), and `BuyTrainerService` (`0x4da210`) tests nothing before sending.
pub(super) fn train_error_log_line(code: u32) -> &'static str {
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
    /// The player's `PLAYER_SKILL_INFO` slots, `Some` while the player object exists: the skill
    /// requirement scans them by line (`0x4d94d6`-`0x4d9524`), and the skill-line name needs the
    /// player (`0x4d91fd`-`0x4d921c`).
    pub(super) skills: Option<&'a [benilla_protocol::messages::PlayerSkillSlot]>,
}

/// `GetTrainerServiceSkillReq`'s flag (`0x4d94d6`-`0x4d9575`): 1 unless the requirement names a
/// line with a row and the player's first slot for it, its value plus the permanent bonus (added
/// only to a nonzero value), falls short of the required value; no player, no requirement and an
/// unknown line keep the default 1 (`0x4d945a`, `0x4d95bc`).
fn skill_req_met(
    wire: &TrainerSpell,
    skill_lines: Option<&SkillLineCatalog>,
    books: &ReqBooks,
) -> bool {
    let Some(slots) = books.skills else {
        return true;
    };
    if wire.req_skill == 0
        || wire.req_skill_value == 0
        || !skill_lines.is_none_or(|c| c.line(wire.req_skill).is_some())
    {
        return true;
    }
    let total = slots
        .iter()
        .find(|s| u32::from(s.skill_id) == wire.req_skill)
        .map_or(0, |s| match s.value {
            0 => 0,
            v => u32::from(v) + u32::from(s.perm_bonus as u16),
        });
    total >= wire.req_skill_value
}

/// `GetTrainerServiceSkillLine`'s name (`0x4d91dc`-`0x4d92c5`), needing the player: the line of
/// `FindSkillLineAbility(spell, race, class)` on the spell the first learn effect (`0x24` or
/// `0x39`) teaches, else on the sent spell, whose miss goes on to the same lookup on the spell
/// numbered by the sent record's third `EffectMiscValue`, the dword before the trigger array
/// that an index of -1 reads (`0x4d9261`).
fn skill_line_name(
    wire_spell: u32,
    spells: &SpellCatalog,
    skill_lines: Option<&SkillLineCatalog>,
    books: &ReqBooks,
) -> Option<String> {
    books.skills?;
    let lines = skill_lines?;
    let (race, class) = books.race_class;
    let d = spells.get(wire_spell)?;
    let named = |spell: u32| {
        spells.get(spell)?;
        let line = lines.spell_to_line(spell, race, class)?;
        lines.line(line).map(|l| l.name.clone())
    };
    let learn = (0..3).find(|&i| {
        matches!(
            d.effects[i],
            SPELL_EFFECT_LEARN_SPELL | SPELL_EFFECT_LEARN_PET_SPELL
        )
    });
    match learn {
        Some(i) => named(d.effect_trigger_spell[i]),
        None => {
            named(wire_spell).or_else(|| u32::try_from(d.effect_misc_value[2]).ok().and_then(named))
        }
    }
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
    // The VM's own `GlobalStrings.lua`, for the group header labels, and its `GetText`.
    strings: benilla_ui::strings::Strings<'_>,
) -> TrainerService {
    let get = &|key: &str| strings.global(key);
    // `taught` feeds only the group key (`0x4d7c60`); the name and subtext are the wire wrapper's
    // own columns (`0x4d8aa0`, `0x4d8b50`), which differ from the taught spell's on every
    // profession-learn row. The wrapper stays the buy id.
    let taught = spells.learned_spell(wire.spell).unwrap_or(wire.spell);
    let display = spells.get(wire.spell);
    let cat = category(wire.state);
    // The flag compares the player's skill value itself, whatever the row's colour (`0x4d9570`).
    let skill_met = skill_req_met(wire, skill_lines, books);
    // A zero line or value, a line `SkillLine.dbc` lacks and no player are all the miss
    // `nil, 0, 1` (`0x4d9487`-`0x4d94ca` → `0x4d95bc`).
    let skill_req = (wire.req_skill_value != 0 && books.skills.is_some())
        .then(|| skill_lines?.line(wire.req_skill))
        .flatten()
        .map(|l| TrainerSkillReq {
            name: l.name.clone(),
            rank: wire.req_skill_value,
            met: skill_met,
        });
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
    let live = live_service(
        wire,
        trainer_type,
        spells,
        skill_lines,
        books,
        icons,
        items,
        ask,
        text,
    );
    TrainerService {
        spell_id: wire.spell,
        name: display.map(|d| d.name.clone()),
        // `0x4d8b50` answers the record's NameSubtext, "" for a rankless spell (`0x4d8b84`), and
        // NULL without a Spell record (`0x4d8b58`), which `0x6f3890` pushes as nil.
        subtext: display.map(|d| d.rank.clone().unwrap_or_default()),
        texture: live.texture,
        icon_wait: live.icon_wait,
        description: live.description.map(|t| t.text(strings)),
        description_wait: live.description_wait,
        cost: wire.cost,
        prof_first_rank: wire.is_primary_prof_first_rank,
        category: cat,
        level_req: u32::from(wire.req_level),
        skill_req,
        ability_reqs: live.ability_reqs,
        is_trade_skill,
        learn_spell,
        group_key,
        group_name,
        tooltip: service_tooltip(wire.spell, spells),
        skill_step: display.is_some_and(|d| d.effects.contains(&SPELL_EFFECT_SKILL_STEP)),
        step_increase: step_increase(wire.spell, spells, skill_lines, get),
        skill_line: skill_line_name(wire.spell, spells, skill_lines, books),
        line_name: skill_lines
            .and_then(|c| c.line(group_key))
            .map(|l| l.name.clone()),
    }
}

/// What the getters read of one service at their call ([`TrainerServiceLive`]): the icon and the
/// description, with the product lookups they miss, and each required ability against the books.
#[allow(clippy::too_many_arguments)] // the reads one service's getters make
fn live_service(
    wire: &TrainerSpell,
    trainer_type: u32,
    spells: &SpellCatalog,
    skill_lines: Option<&SkillLineCatalog>,
    books: &ReqBooks,
    icons: Option<&ItemDisplays>,
    items: &Items,
    ask: Ask<'_>,
    text: &ServiceText,
) -> TrainerServiceLive {
    let display = spells.get(wire.spell);
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
    TrainerServiceLive {
        texture,
        icon_wait,
        description,
        description_wait,
        ability_reqs,
        skill_met: skill_req_met(wire, skill_lines, books),
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
    strings: benilla_ui::strings::Strings<'_>,
) -> Option<TrainerState> {
    let get = |key: &str| strings.global(key);
    let guid = open.listed?;
    Some(TrainerState {
        guid,
        list: open.list,
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
                    strings,
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

/// The player the state pass reads: its descriptor, the stores that resolve its pet, and the pet
/// bar; each absent in a harness without it.
#[derive(SystemParam)]
pub(crate) struct PlayerReads<'w, 's> {
    self_player: Query<'w, 's, (Entity, &'static ObjectStore), With<SelfPlayer>>,
    stores: Query<'w, 's, &'static ObjectStore>,
    index: Option<Res<'w, GuidIndex>>,
    pet_bar: Option<Res<'w, crate::ui_pet::PetBar>>,
}

impl PlayerReads<'_, '_> {
    fn me(&self) -> Option<&ObjectStore> {
        self.self_player.iter().next().map(|(_, store)| store)
    }

    fn store(&self, guid: u64) -> Option<&ObjectStore> {
        let entity = *self.index.as_deref()?.0.get(&guid)?;
        self.stores.get(entity).ok()
    }

    /// What `0x4d7d40` reads off the player: its book, level and skill slots and its summon.
    fn view(&self, known: &BTreeSet<u32>) -> reeval::PlayerView {
        let me = self.me();
        let empty = benilla_protocol::messages::PetSpells::default();
        let bar = self.pet_bar.as_deref().map_or(&empty, |b| &b.spells);
        reeval::PlayerView {
            known: known.clone(),
            level: me.and_then(|s| s.0.unit_level()).unwrap_or(0),
            skills: me
                .map(|store| {
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
            pet: pet_view(me.and_then(|s| s.0.unit_summon()), |g| self.store(g), bar),
        }
    }
}

/// The pet a pet-learn row re-evaluates against: the unit the player's `UNIT_FIELD_SUMMON` names
/// (`0x4d7f2d`, looked up with mask 8 at `0x4d7f49`), `None` when none is held (`0x4d7f5a`), at its
/// own level (`0x4d7f99`). `KnowsSpell` (`0x60c740`) reads a unit's book only as the pet bar's
/// unit (`0x60c77a`-`0x60c790`), so another summon knows nothing.
fn pet_view<'a>(
    summon: Option<u64>,
    store_of: impl Fn(u64) -> Option<&'a ObjectStore>,
    bar: &benilla_protocol::messages::PetSpells,
) -> Option<reeval::PetView> {
    /// `TYPEMASK_UNIT`, the lookup's mask.
    const TYPEMASK_UNIT: u32 = 0x8;
    let guid = summon?;
    let store = store_of(guid)
        .filter(|store| store.0.object_type_mask().unwrap_or(0) & TYPEMASK_UNIT != 0)?;
    let known = if guid == bar.pet_guid {
        bar.spells
            .iter()
            .filter(|e| e.is_spell())
            .map(|e| e.action())
            .collect()
    } else {
        BTreeSet::new()
    };
    Some(reeval::PetView {
        level: store.0.unit_level().unwrap_or(0),
        known,
    })
}

/// The resources a service description's `$`-tokens read beside the caster's descriptor: the
/// spell-modifier tables and the bind point `$z` names, the same two reads the item and spell
/// tooltips make.
#[derive(SystemParam)]
pub(crate) struct DescriptionReads<'w> {
    spell_mods: Option<Res<'w, crate::spell::SpellModifiers>>,
    home_bind: Option<Res<'w, crate::net::HomeBind>>,
    area_names: Option<Res<'w, crate::ui_quest_log::QuestHeaderNamesRes>>,
}

/// What a service's resolve reads, built once over the world ([`TrainerReads::with`]).
struct Resolver<'a, 't> {
    spells: &'a SpellCatalog,
    skill_lines: &'a SkillLineCatalog,
    books: &'a ReqBooks<'a>,
    icons: Option<&'a ItemDisplays>,
    items: &'a Items,
    text: &'a ServiceText<'a, 't>,
}

/// Everything a list's or a pass's push and a getter's per-call read resolve the services from:
/// the catalogs, the books, the item caches and the casters the descriptions expand against.
#[derive(SystemParam)]
pub(crate) struct TrainerReads<'w, 's> {
    player: PlayerReads<'w, 's>,
    actions: Option<Res<'w, PlayerActions>>,
    spells: Option<Res<'w, Spells>>,
    skill_lines: Option<Res<'w, SkillLines>>,
    desc: DescriptionReads<'w>,
    // A tradeskill row shows its created item's icon: the template cache and `ItemDisplayInfo.dbc`.
    icons: Option<Res<'w, ItemDisplays>>,
    items: Option<Res<'w, Items>>,
}

impl TrainerReads<'_, '_> {
    /// `f` over the world's resolve: `None` until `Spell.dbc` and the skill lines load, without
    /// which every service would group to 0 and drop.
    fn with<R>(&self, f: impl FnOnce(&Resolver) -> R) -> Option<R> {
        let spells = self.spells.as_deref()?;
        let skill_lines = self.skill_lines.as_deref()?;
        let items = self.items.as_deref()?;
        let actions = self.actions.as_deref()?;
        let mods = self.desc.spell_mods.as_deref()?;
        // The spell tooltip's description reads (`0x52f717`), and `0x6e3159`'s charm-else-summon
        // for a `LEARN_PET_SPELL` row.
        let store = self.player.me();
        let pet = store
            .and_then(|s| s.0.unit_pet_guid())
            .and_then(|guid| self.player.store(guid));
        let home_area: Option<&str> = self
            .desc
            .home_bind
            .as_deref()
            .and_then(|b| b.0)
            .and_then(|id| self.desc.area_names.as_deref()?.0.resolve(id as i32));
        let skills = crate::spell::PlayerSkills {
            lines: Some(&skill_lines.catalog),
            spells: Some(&spells.catalog),
            items: Some(items),
        };
        // `UNIT_FIELD_SUMMON`'s unit (`0x4d9860`-`0x4d987d`), its book read only as the pet bar's
        // (`0x60c77a`).
        let pet_book: Option<BTreeSet<u32>> = store
            .and_then(|s| s.0.unit_summon())
            .filter(|&guid| self.player.store(guid).is_some())
            .map(|guid| match self.player.pet_bar.as_deref() {
                Some(bar) if bar.spells.pet_guid == guid => bar
                    .spells
                    .spells
                    .iter()
                    .filter(|e| e.is_spell())
                    .map(|e| e.action())
                    .collect(),
                _ => BTreeSet::new(),
            });
        let skill_slots: Option<Vec<_>> = store.map(|s| {
            (0..PLAYER_SKILL_SLOTS)
                .filter_map(|i| s.0.player_skill(i))
                .collect()
        });
        let books = ReqBooks {
            player: &actions.spells,
            pet: pet_book.as_ref(),
            race_class: store.map_or((0, 0), |s| s.race_class()),
            skills: skill_slots.as_deref(),
        };
        Some(crate::spell::with_token_contexts(
            spells,
            skills,
            mods,
            (store, pet),
            home_area,
            |(player, pet)| {
                f(&Resolver {
                    spells: &spells.catalog,
                    skill_lines: &skill_lines.catalog,
                    books: &books,
                    icons: self.icons.as_deref(),
                    items,
                    text: &ServiceText { player, pet },
                })
            },
        ))
    }

    /// The window's services resolved for the VM, the header labels read through `get`; `None`
    /// before the catalogs load.
    fn snapshot(
        &self,
        open: &TrainerOpen,
        strings: benilla_ui::strings::Strings<'_>,
    ) -> Option<Option<TrainerState>> {
        self.with(|r| {
            snapshot(
                open,
                r.spells,
                Some(r.skill_lines),
                r.books,
                r.icons,
                r.items,
                // A product is asked by the icon and description getters' calls alone, inside
                // them (`0x4d9096`, `0x4d9d07`): the list and its passes look no item up.
                Ask::Read,
                r.text,
                strings,
            )
        })
    }

    /// The getters' per-call reads of the service at `service`, as the world holds them now.
    fn live(&self, open: &TrainerOpen, service: usize) -> Option<TrainerServiceLive> {
        let wire = open.services.get(service)?;
        self.with(|r| {
            live_service(
                wire,
                open.trainer_type,
                r.spells,
                Some(r.skill_lines),
                r.books,
                r.icons,
                r.items,
                Ask::Read,
                r.text,
            )
        })
    }

    /// The state pass `0x4d7d40` over every row of `open`, as its re-derive (`0x4d7d46`-`0x4d8252`)
    /// and its known-key writes (`0x4d82b2`-`0x4d82c1`); whether the catalogs let it run.
    fn pass(&self, open: &mut TrainerOpen) -> bool {
        let (Some(spells), Some(skill_lines), Some(actions)) = (
            self.spells.as_deref(),
            self.skill_lines.as_deref(),
            self.actions.as_deref(),
        ) else {
            return false;
        };
        let player = self.player.view(&actions.spells);
        let TrainerOpen {
            services,
            trainer_type,
            known_key,
            ..
        } = open;
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
        true
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
/// the cell and the VM's rows cleared (`0x4d747c`-`0x4d7486`). The fire comes before the clear, so
/// `OnHide`'s `CloseTrainer()` re-enters with the cell set; behind what the lane holds, it fires
/// after the list it closes, and the rows clear after its signals.
pub(crate) fn close_core(world: &mut World) {
    if world.resource::<TrainerOpen>().trainer.is_none() {
        return;
    }
    let ran = lane::behind_lane(world, |world| {
        crate::ui_session::fire(world, |script| {
            script.fire_event("TRAINER_CLOSED", vec![]);
        });
        if let Some(trainer) = world.resource::<TrainerOpen>().trainer {
            crate::ui_session::release(world, trainer);
        }
        if world.resource::<TrainerOpen>().trainer.is_some() {
            world.resource_mut::<TrainerOpen>().clear();
        }
    });
    match ran {
        crate::lanes::Behind::Ran => {
            crate::ui_session::with_script(world, |script| script.set_trainer(None));
        }
        crate::lanes::Behind::Parked => {
            world
                .resource_mut::<TrainerLane>()
                .push(TrainerEvent::Cleared);
        }
        // The outer core clears them after every signal.
        crate::lanes::Behind::Nested => {}
    }
}

impl TrainerOpen {
    /// The world-enter cascade's trainer reset (`0x4909ab` → `0x4d73e0`), which signals nothing:
    /// the trainer's guid, selection, total and greeting zeroed (`0x4d73e2`-`0x4d73f6`), so the
    /// getters find no services. The sweep ahead of every cascade has closed the window, so only
    /// the records a close leaves go here. Owed from the cascade's push, it runs at the cascade's
    /// place or ahead of a later list ([`crate::ui_world_enter::OwedReset`]).
    pub(crate) fn settle_world_reset(&mut self) {
        if self.world_reset.take() {
            self.trainer = None;
            self.services.clear();
            self.greeting.clear();
        }
    }
}

/// The trainer reset in the cascade's place, unless a later list ran it first: the engine's
/// records, then the VM's rows and selection, its collapsed lines kept
/// ([`UiScript::reset_trainer_world_state`]).
pub(crate) fn reset_on_world_enter(
    mut vm: crate::script_calls::VmWith<Option<ResMut<'static, TrainerOpen>>>,
) {
    if let Some(mut open) = vm.own() {
        open.settle_world_reset();
    }
    vm.run(|script| {
        if let Some(script) = script {
            script.reset_trainer_world_state();
        }
    });
}

/// A `/reload`'s VM gets the window the engine holds (`0xb737b0`, `0xb73a10`), rows and all, with
/// no `TRAINER_SHOW`: the list's resets, then its push and sort.
pub(crate) fn seed_reloaded_vm(world: &mut World, script: &mut UiScript) {
    if world
        .get_resource::<TrainerOpen>()
        .is_none_or(|open| open.trainer.is_none())
    {
        return;
    }
    let mut reads = bevy::ecs::system::SystemState::<(TrainerReads, Res<TrainerOpen>)>::new(world);
    let (reads, open) = reads.get(world);
    let state = {
        let lua = script.lua();
        reads.snapshot(&open, benilla_ui::strings::Strings::Vm(lua))
    };
    if let Some(state) = state {
        script.reset_trainer_list_state(open.trainer_type);
        script.set_trainer(state);
    }
}

/// `SMSG_TRAINER_LIST`'s open, `0x4d7440(trainer)`: the open trainer's own list re-lists in place
/// (`0x4d7459`); a zero guid is the close core (`0x4d7461`), after which the list builds on the
/// zero cell; any other acknowledges its tutorial, Learning Talents for our own guid and Trainers
/// for another (`0x4d7492`-`0x4d74ae`), registers on the latch (`0x4d74ce`), then the cell
/// (`0x4d74d5`). The builder's `TRAINER_SHOW` (`0x4d7b73`) goes on the lane.
pub(crate) fn open_trainer(
    world: &mut World,
    trainer: u64,
    trainer_type: u32,
    services: Vec<TrainerSpell>,
    greeting: String,
) {
    if trainer == 0 {
        close_core(world);
    } else if world.resource::<TrainerOpen>().trainer != Some(trainer) {
        let own = world
            .get_resource::<crate::net::SelfGuid>()
            .and_then(|g| g.0)
            == Some(trainer);
        if let Some(tutorials) = world.get_resource::<crate::tutorial::Tutorials>() {
            use crate::tutorial::id;
            tutorials.acknowledge_at_place(if own {
                id::LEARNING_TALENTS
            } else {
                id::TRAINERS
            });
        }
        crate::ui_session::register(world, trainer);
    }
    world
        .resource_mut::<TrainerOpen>()
        .open(trainer, trainer_type, services, greeting);
    if let Some(mut lane) = world.get_resource_mut::<TrainerLane>() {
        lane.push(TrainerEvent::Show);
    }
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
    /// (`0x5df6bb`): the asked guid (`0x5df69f`), then `CMSG_TRAINER_LIST` (`0x5df6fc`), which the
    /// server answers with nothing for a player.
    pub(crate) fn ask_own_list(&mut self, trainer: u64) {
        debug!("ui_trainer: OpenTrainer asks for the player's own list");
        if let Some(open) = self.open.as_deref_mut() {
            open.asked = trainer;
        }
        let _ = self.net.0.send(ClientCommand::TrainerList { trainer });
    }

    /// The trainer's cell `[0xb73a00]`.
    pub(crate) fn trainer(&self) -> Option<u64> {
        self.open.as_deref().and_then(|o| o.trainer)
    }

    /// The close core's clear of the cell (`0x4d747c`), and the VM's window with it, when a
    /// trainer is stored.
    pub(crate) fn clear_trainer(&mut self, script: &mut UiScript) {
        let Some(open) = self.open.as_deref_mut().filter(|o| o.trainer.is_some()) else {
            return;
        };
        open.clear();
        script.set_trainer(None);
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
