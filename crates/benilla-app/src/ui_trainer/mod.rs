//! The trainer feed: [`TrainerOpen`] from `SMSG_TRAINER_LIST`, [`feed_trainer`] resolving it into
//! Lua [`TrainerService`]s and firing the trainer events, and [`drain_trainer`] sending the buys;
//! `CloseTrainer` sends nothing. A bought row repaints through the re-evaluator ([`reeval`]),
//! never through `SMSG_TRAINER_BUY_SUCCEEDED`, and a refusal goes only to the log.

use std::collections::BTreeSet;

use benilla_formats::{SkillLineCatalog, SpellCatalog};
use benilla_protocol::messages::TrainerSpell;
use bevy::prelude::*;

use benilla_ui::script::{
    ScriptValue, TrainerAbilityReq, TrainerService, TrainerServiceCategory, TrainerSkillReq,
    TrainerState, UiScript,
};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::names::NameCache;
use benilla_protocol::field::{
    FIELD_PLAYER_FIELD_COINAGE, FIELD_PLAYER_SKILL_INFO_1_1, FIELD_UNIT_LEVEL,
};
use benilla_protocol::messages::PLAYER_SKILL_SLOTS;
use bevy::ecs::system::SystemParam;

use crate::net::{ClientCommand, FieldChanged, GuidIndex, NetCommands, ObjectStore, SelfPlayer};
use crate::ui_action::{PlayerActions, Spells};
use crate::ui_script::{UiFeed, UiInput};
use crate::ui_session::{close_npc_session_out_of_range, npc_switched, NpcSession};
use crate::ui_spellbook::SkillLines;

/// `SMSG_TRAINER_LIST`'s `trainer_type` for a tradeskill trainer (0 class, 1 mount, 2 tradeskill,
/// 3 pet). `IsTrainerServiceTradeSkill` answers it for every service, where the 1.12 verb
/// (`0x4d9f70`) answers per service; the stock window never calls it.
const TRAINER_TYPE_TRADESKILL: u32 = 2;
/// A mount trainer, which the client calls "talent" (`IsTalentTrainer 0x4d8ed0`).
const TRAINER_TYPE_MOUNT: u32 = 1;

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
    /// One of the reference's twelve re-evaluation triggers fired with the window open; the feed
    /// re-derives and fires `TRAINER_UPDATE` even if no state moved, as `0x4d7d40`'s tail does
    /// (`0x4d83f4`), and the stock window re-reads `GetMoney()` on it.
    pub(crate) re_derive: bool,
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
        self.services = services;
        self.greeting = greeting;
        // A fresh list carries the server's own states, so nothing is pending against it.
        self.fresh_list = true;
        self.re_derive = false;
    }

    /// One of the twelve triggers fired: re-derive on the next feed, if a window is open.
    pub(crate) fn trigger_re_derive(&mut self) {
        if self.trainer.is_some() {
            self.re_derive = true;
        }
    }

    /// A client-side close; a re-open re-lists.
    pub(crate) fn clear(&mut self) {
        self.trainer = None;
        self.services.clear();
        self.trainer_type = 0;
        self.greeting.clear();
        self.fresh_list = false;
        self.re_derive = false;
    }

    /// Disconnect: drop the open window.
    pub(crate) fn clear_session(&mut self) {
        self.clear();
    }
}

/// `SMSG_TRAINER_BUY_FAILED` codes, queued for the feed to log.
#[derive(Resource, Default)]
pub(crate) struct TrainerErrors(pub Vec<u32>);

mod net;
mod reeval;

pub(crate) struct UiTrainerPlugin;

impl Plugin for UiTrainerPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<TrainerOpen>()
            .init_resource::<TrainerErrors>()
            .add_systems(
                Update,
                (
                    // Range-close before the feed so the clear fires TRAINER_CLOSED the same frame.
                    close_npc_session_out_of_range::<TrainerOpen>.before(feed_trainer),
                    // The unit feeds also write the pet bar the re-evaluator reads; either order
                    // reads the same pet spellbook, but the schedule check wants one declared.
                    feed_trainer.in_set(UiFeed).after(crate::ui_unit::UnitFeed),
                    drain_trainer.after(UiInput),
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

/// Resolve one wire [`TrainerSpell`] into the Lua [`TrainerService`]; `known` is the player's
/// known-spell set, which colours each prerequisite ability.
fn resolve_service(
    wire: &TrainerSpell,
    trainer_type: u32,
    spells: &SpellCatalog,
    skill_lines: Option<&SkillLineCatalog>,
    known: &BTreeSet<u32>,
    icons: Option<&ItemDisplays>,
    items: &Items,
    commands: &NetCommands,
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
    // `GetTrainerServiceAbilityReq`'s hasReq is `IsSpellKnown` per requirement, apart from the
    // category (`0x4d96e0`), on a real ability id with no hop; the client also ORs
    // `KnownHigherRank`, which this does not.
    let ability_reqs = wire
        .req_spells
        .iter()
        .filter(|&&s| s != 0)
        .map(|&s| {
            let name = match spells.get(s) {
                Some(d) => d.ranked_name(),
                None => format!("Spell {s}"),
            };
            TrainerAbilityReq {
                name,
                met: known.contains(&s),
            }
        })
        .collect();
    // The wrapper is never in `SkillLineAbility`, so the skill-line arm keys on the taught spell;
    // a service it cannot place drops, as in the client's builder, and is logged.
    let (group_key, group_name) = service_group(
        wire.spell,
        taught,
        trainer_type,
        cat,
        spells,
        skill_lines,
        get,
    );
    if group_key == 0 && skill_lines.is_some() {
        debug!(
            "ui_trainer: trainer spell {} (teaches {taught}) has no skill line — dropped from the tree",
            wire.spell
        );
    }
    TrainerService {
        spell_id: wire.spell,
        name: display.map(|d| d.name.clone()),
        subtext: display.and_then(|d| d.rank.clone()),
        // Over the wire spell, like the name and subtext ([`service_icon`]).
        texture: service_icon(wire.spell, trainer_type, spells, icons, items, commands),
        // Empty: the 1.12 `GetTrainerServiceDescription` returns `Spell.dbc`'s Description with
        // its `$s1`/`$o1`/`$d`/`$a1` tokens substituted, and that substitution is not built.
        description: String::new(),
        cost: wire.cost,
        prof_first_rank: wire.is_primary_prof_first_rank,
        category: cat,
        level_req: u32::from(wire.req_level),
        skill_req,
        ability_reqs,
        is_trade_skill: trainer_type == TRAINER_TYPE_TRADESKILL,
        group_key,
        group_name,
        tooltip: service_tooltip(wire.spell, spells),
    }
}

/// Build the Lua snapshot from [`TrainerOpen`] and the spell and skill catalogs.
fn snapshot(
    open: &TrainerOpen,
    spells: &SpellCatalog,
    skill_lines: Option<&SkillLineCatalog>,
    known: &BTreeSet<u32>,
    icons: Option<&ItemDisplays>,
    items: &Items,
    commands: &NetCommands,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<TrainerState> {
    open.trainer?;
    Some(TrainerState {
        greeting: open.greeting.clone(),
        trainer_type: open.trainer_type,
        services: open
            .services
            .iter()
            .map(|w| {
                resolve_service(
                    w,
                    open.trainer_type,
                    spells,
                    skill_lines,
                    known,
                    icons,
                    items,
                    commands,
                    get,
                )
            })
            .collect(),
        // The engine builds the tree in `set_trainer` from each service's group key.
        groups: Vec::new(),
    })
}

/// The re-evaluator's inputs and its descriptor triggers: the player's descriptor and its field
/// edges (money, level, a skill slot), the pet bar, and the stores that resolve the pet.
#[derive(SystemParam)]
struct ReEvalInputs<'w, 's> {
    self_player: Query<'w, 's, (Entity, &'static ObjectStore), With<SelfPlayer>>,
    stores: Query<'w, 's, &'static ObjectStore>,
    index: Res<'w, GuidIndex>,
    pet_bar: Res<'w, crate::ui_pet::PetBar>,
    edges: MessageReader<'w, 's, FieldChanged>,
}

/// Push the current trainer into the VM and fire its events on a change. Another trainer while
/// open is a close then an open: `ShowUIPanel` returns early on a visible frame.
#[allow(clippy::too_many_arguments)] // one Bevy system's full input set
fn feed_trainer(
    script: Option<NonSendMut<UiScript>>,
    // Writes the re-derived states, as `0x4d7d40` overwrites its own records.
    mut open: ResMut<TrainerOpen>,
    actions: Res<PlayerActions>,
    spells: Option<Res<Spells>>,
    skill_lines: Option<Res<SkillLines>>,
    mut re_eval: ReEvalInputs,
    // A tradeskill row shows its created item's icon: the template cache and `ItemDisplayInfo.dbc`.
    icons: Option<Res<ItemDisplays>>,
    items: Res<Items>,
    mut errors: ResMut<TrainerErrors>,
    commands: Res<NetCommands>,
    names: Res<NameCache>,
    mut last: Local<crate::ui_script::VmMemo<Option<TrainerState>>>,
    mut last_trainer: Local<crate::ui_script::VmMemo<Option<u64>>>,
    mut last_name: Local<crate::ui_script::VmMemo<Option<String>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let last = last.get(&script);
    let last_trainer = last_trainer.get(&script);
    let last_name = last_name.get(&script);
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
    // The per-list reset goes in before the snapshot, so `TRAINER_SHOW` finds it; the stock window
    // re-applies its saved filter only on its own `ADDON_LOADED`.
    if open.fresh_list {
        script.reset_trainer_list_state(open.trainer_type);
        open.fresh_list = false;
    }
    // The player's money, level and skill triggers, read every frame so the reader never backs up.
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
    // `0x4d7d40` over every row, then `TRAINER_UPDATE` unconditionally, as its tail fires it.
    let forced = open.re_derive && open.trainer.is_some();
    if forced {
        open.re_derive = false;
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
            ..
        } = &mut *open;
        reeval::re_derive_all(
            services,
            *trainer_type,
            &spells.catalog,
            &skill_lines.catalog,
            &player,
        );
    }
    let fresh = snapshot(
        &open,
        &spells.catalog,
        Some(&skill_lines.catalog),
        &actions.spells,
        icons.as_deref(),
        &items,
        &commands,
        &|key: &str| {
            script
                .lua()
                .globals()
                .get::<String>(key)
                .ok()
                .filter(|t| !t.is_empty())
        },
    );
    // A name-only change re-fires `TRAINER_UPDATE`, so the title's `UnitName("npc")` repaints.
    // The name rides as arg1, which the 1.12 trainer events do not carry.
    let trainer_name = open
        .trainer
        .and_then(|g| names.resolve(g, &commands).map(str::to_string));
    let name_changed = *last_name != trainer_name;
    let switched = npc_switched(*last_trainer, open.trainer);
    if fresh == *last && !name_changed && !switched && !forced {
        return;
    }
    script.set_trainer(fresh.clone());
    let name_arg = || vec![ScriptValue::Str(trainer_name.clone().unwrap_or_default())];
    if switched {
        // Drop the close intent `OnHide` queues, or the drain would clear the new trainer.
        script.fire_event("TRAINER_CLOSED", vec![]);
        script.fire_event("TRAINER_SHOW", name_arg());
        let _ = script.take_trainer_close();
    } else {
        match (&*last, &fresh) {
            (None, Some(_)) => script.fire_event("TRAINER_SHOW", name_arg()),
            (Some(_), Some(_)) => script.fire_event("TRAINER_UPDATE", name_arg()),
            (Some(_), None) => script.fire_event("TRAINER_CLOSED", vec![]),
            (None, None) => {}
        }
    }
    *last = fresh;
    *last_trainer = open.trainer;
    *last_name = trainer_name;
}

/// The range guard closes the window, as `CloseTrainer` would, on walk-away or despawn.
impl NpcSession for TrainerOpen {
    fn npc(&self) -> Option<u64> {
        self.trainer
    }

    fn close(&mut self) {
        self.clear();
    }
}

/// Each `BuyTrainerService` becomes `CMSG_TRAINER_BUY_SPELL`; a close sends nothing.
fn drain_trainer(
    script: Option<NonSendMut<UiScript>>,
    mut open: ResMut<TrainerOpen>,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    for spell_id in script.take_trainer_buys() {
        let Some(trainer) = open.trainer else {
            debug!(
                "ui_trainer: BuyTrainerService(spell {spell_id}) with no open trainer — ignored"
            );
            continue;
        };
        debug!("ui_trainer: buy service (trainer {trainer:#x}, spell {spell_id})");
        let _ = commands
            .0
            .send(ClientCommand::TrainerBuySpell { trainer, spell_id });
    }
    if script.take_trainer_close() {
        debug!("ui_trainer: client-side close (no packet)");
        open.clear();
    }
}

mod law;
use law::{category, service_group, service_icon, service_tooltip};

#[cfg(test)]
mod tests;
