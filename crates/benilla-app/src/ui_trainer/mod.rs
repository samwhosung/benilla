//! The app-side **trainer feed** (decision 0237 phase 3) — the inward half of the trainer seam
//! around [`benilla_ui::script`]'s `trainer` module, the twin of [`crate::ui_merchant`]'s merchant
//! feed.
//!
//! The net bridge fills [`TrainerOpen`] from the wire (`SMSG_TRAINER_LIST` → the trainer's services +
//! greeting, reached through the gossip trainer option). Each frame [`feed_trainer`] resolves each
//! wire [`TrainerSpell`] to a Lua-facing [`TrainerService`] — name/subtext from the spell catalog
//! (`Spell.dbc`, loaded whole at startup), the **icon** from its own byte-verified law
//! ([`service_icon`], which at a *tradeskill* trainer fronts the taught recipe's created item and so
//! does need the ask-once item-template query the item rows use), the skill-requirement name from
//! the skill-line catalog, the green/red/gray state straight off the wire
//! `state` byte — pushes the snapshot ([`UiScript::set_trainer`]), and fires `TRAINER_SHOW` on open /
//! `TRAINER_UPDATE` on a content change / `TRAINER_CLOSED` on clear. [`drain_trainer`] pulls the Lua
//! intents back out: the Train button's `BuyTrainerService` → `CMSG_TRAINER_BUY_SPELL` for the open
//! trainer, and `CloseTrainer` → a local clear (vanilla's client-side close sends no packet). A
//! successful buy answers `SMSG_TRAINER_BUY_SUCCEEDED`, which the reference does not even handle:
//! the spell itself lands via `SMSG_LEARNED_SPELL`, and that — like a level, a skill point, a coin
//! or a pet spell — runs the **state re-evaluator** ([`reeval`], the reference's `0x4d7d40`,
//! decision 2333) over the open list and fires `TRAINER_UPDATE`; a refusal
//! (`SMSG_TRAINER_BUY_FAILED`) stages into [`TrainerErrors`] for the feed to surface on the
//! window's red error line.
//!
//! The standardized NPC-session range guard ([`crate::ui_session`]) client-side-closes the window
//! when the player walks out of the trainer's service range (or the trainer despawns) — the same
//! `CloseTrainer` clear.

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

/// `SMSG_TRAINER_LIST`'s `trainer_type` for a tradeskill trainer (0 class · 1 mount · 2 tradeskill ·
/// 3 pet). Three separate laws fork on it here — the **icon** ([`service_icon`]), the **group key**
/// ([`service_group`]), and the Era `IsTrainerServiceTradeSkill` flag, which remains a whole-trainer
/// approximation (per-service typing from the spell's effects is a later refinement; the reference
/// window's Lua never calls it).
const TRAINER_TYPE_TRADESKILL: u32 = 2;
/// `SMSG_TRAINER_LIST`'s `trainer_type` for a mount trainer — the type the client's own vocabulary
/// calls "talent" (`IsTalentTrainer 0x4d8ed0`), and the one whose grouping folds already-known
/// services into a "My Talents" bucket ([`service_group`], decision 1124).
const TRAINER_TYPE_MOUNT: u32 = 1;

/// The open trainer, filled by the net bridge ([`crate::net`]) and read by [`feed_trainer`]. Holds
/// the trainer guid and its services exactly as the wire delivered them (`SMSG_TRAINER_LIST`), plus
/// the window-framing type and the greeting title. Cleared on a client-side close and on disconnect.
#[derive(Resource, Default)]
pub(crate) struct TrainerOpen {
    /// The trainer whose window is open; `None` = no trainer open.
    pub(crate) trainer: Option<u64>,
    /// The wire services (order = 1-based display order).
    pub(crate) services: Vec<TrainerSpell>,
    /// The window-framing kind (0 class · 1 mount · 2 tradeskill · 3 pet).
    pub(crate) trainer_type: u32,
    /// The trainer's greeting line (`SMSG_TRAINER_LIST`'s trailing string).
    pub(crate) greeting: String,
    /// A `SMSG_TRAINER_LIST` has landed and the feed has not yet handed it to the engine. Drives
    /// the engine's filter/collapse/**selection** reset ([`UiScript::reset_trainer_list_state`]) —
    /// the reference's builder rewrites all three on every list packet (decision 1128; 2231 for
    /// the selection, `0x4d7b42`). It is not the same edge as a snapshot change: those re-push
    /// the same list.
    ///
    /// Every list packet begins a window session, in benilla as in the reference: the reference
    /// gets one only when a trainer opens, and since 2333 so does benilla — a purchase, a level
    /// or a skill point repaints the open window through the state re-evaluator ([`reeval`]),
    /// never through a second list. (B253/B256's arc, when benilla still re-requested the list
    /// after every buy, needed a "this packet is our own refresh" mark here so the player's
    /// filter survived a purchase; the re-request and the mark went together.)
    pub(crate) fresh_list: bool,
    /// One of the reference's twelve re-evaluation triggers fired while the window is open — a
    /// spell learned/removed/superseded, a spell modifier (the two "talent" callers, 2336), the
    /// pet's spellbook, or (read by the feed itself off the descriptor edges) money, level or a
    /// skill slot. The feed runs [`reeval::re_derive_all`]
    /// over the list and fires `TRAINER_UPDATE` whether or not a state moved, as `0x4d7d40`'s
    /// tail does (`0x4d83f4`) — the stock window re-reads `GetMoney()` for the Train button on
    /// exactly that event.
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
        // Every list packet is a window session (the field's doc says why), and a fresh list is
        // the server's own states: nothing is pending against it.
        self.fresh_list = true;
        self.re_derive = false;
    }

    /// One of the twelve triggers fired: re-derive on the next feed, if a window is open.
    pub(crate) fn trigger_re_derive(&mut self) {
        if self.trainer.is_some() {
            self.re_derive = true;
        }
    }

    /// Close the open window (a client-side close). Keeps nothing — a re-open re-lists.
    pub(crate) fn clear(&mut self) {
        self.trainer = None;
        self.services.clear();
        self.trainer_type = 0;
        self.greeting.clear();
        self.fresh_list = false;
        self.re_derive = false;
    }

    /// Disconnect: drop the open window (mirrors the gossip/merchant session clears).
    pub(crate) fn clear_session(&mut self) {
        self.clear();
    }
}

/// Trainer purchase refusals (`SMSG_TRAINER_BUY_FAILED`), staged by the net apply for the feed to
/// surface on the window's red error line — the merchant [`crate::ui_merchant::MerchantErrors`]
/// twin. Each entry is a [`benilla_protocol::messages::train_fail`] code.
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
                    // Range-close before the feed so the clear turns into TRAINER_CLOSED the same frame;
                    // push before the input pass so an open/close is on screen the same frame; drain
                    // after it (mirrors ui_merchant/ui_gossip).
                    close_npc_session_out_of_range::<TrainerOpen>.before(feed_trainer),
                    // After the unit feeds: the re-evaluator reads the pet bar (2333), which the
                    // pet's old-target clear (a `UnitFeed` member) also writes — the same shape
                    // `ui_script`'s demo feed takes. Either order reads the same pet spellbook;
                    // the declaration is what the schedule ratchet asks for (2287).
                    feed_trainer.in_set(UiFeed).after(crate::ui_unit::UnitFeed),
                    drain_trainer.after(UiInput),
                ),
            );
    }
}

/// A trainer refusal (`SMSG_TRAINER_BUY_FAILED`'s
/// [`benilla_protocol::messages::train_fail`] code) is **silent on every player-facing surface**,
/// and goes to the log instead. That is the reference's own behaviour (handler `0x5e5f10`,
/// registered at `0x5e3291`, arms at `0x5e5f95` / `0x5e5fb8` / `0x5e5fdc`, a code ≥ 3 doing
/// nothing at all):
///
/// - there is **no GlobalStrings key and no message-catalog id** — the handler passes neither;
/// - there is **no `DisplayError 0x496720` and no `0x4945b0`** anywhere in its body, so the line
///   never reaches `UIErrorsFrame`, the info line, or chat;
/// - all three arms call **`0x63cd00`**, the developer console's printf, with a hardcoded C format
///   string whose `%d` is the spell id — the strings below, which are the *client's own* and are
///   each referenced exactly once image-wide (`0x8604bc`, `0x860494`, `0x860464`);
/// - no Lua event fires either: the reference UI knows only `TRAINER_SHOW`/`TRAINER_CLOSED`/
///   `TRAINER_UPDATE`, and `BuyTrainerService 0x4da210` runs no client-side money or skill test
///   before sending, so there is no second path that would speak.
///
/// benilla showed a red `UI_ERROR_MESSAGE` here, one of whose three sentences was a re-typed
/// `ERR_NOT_ENOUGH_MONEY` and two of which were invented (decision 2045). Both problems have the
/// same answer, and it is the reference's: say it to the log and nothing to the player. Rarely
/// reached either way — the Train button is disabled unless the service is available and
/// affordable.
fn train_error_log_line(code: u32) -> &'static str {
    use benilla_protocol::messages::train_fail;
    match code {
        train_fail::NOT_ENOUGH_MONEY => "Not enough money for trainer service",
        train_fail::NOT_ENOUGH_SKILL => "Not enough skill points for trainer service",
        _ => "Trainer service unavailable",
    }
}

/// Resolve one wire [`TrainerSpell`] into the Lua-facing [`TrainerService`]: name/subtext/icon from
/// the spell catalog (`None` only before `Spell.dbc` has loaded — the row shows a placeholder), the
/// skill-req name from the skill-line catalog (falling back to `Skill <id>` if it hasn't loaded),
/// the ability-req names + rank from the same spell catalog, the state/cost/gates straight off the
/// wire. `known` is the player's known-spell set ([`PlayerActions::spells`]) — each prerequisite
/// ability is coloured by whether the player already knows that specific spell (see below).
fn resolve_service(
    wire: &TrainerSpell,
    trainer_type: u32,
    spells: &SpellCatalog,
    skill_lines: Option<&SkillLineCatalog>,
    known: &BTreeSet<u32>,
    icons: Option<&ItemDisplays>,
    items: &Items,
    commands: &NetCommands,
    // The VM's own `GlobalStrings.lua`, for [`service_group`]'s three header labels.
    get: &dyn Fn(&str) -> Option<String>,
) -> TrainerService {
    // The trainer offers a LEARN wrapper (decision 0247); the ability it teaches is the taught
    // spell, and the tree GROUPS by that hop (`0x4d7c60` → `[skillrec+4]`). It is the only thing
    // that hops: the row's **displayed name and subtext are the WIRE spell's own** `Spell.dbc`
    // columns — `GetTrainerServiceInfo` reads `row[+0]` for both returns (`0x4d8aa0` → `[+0x1e0]`,
    // `0x4d8b50` → `[+0x204]`), with no `EffectTriggerSpell` deref anywhere in either body. 0247
    // recorded the display as hopping too; decision 1124 refutes that at the bytes, and it is not a
    // cosmetic difference: 1607 of the 4711 shipped learn wrappers (34.1 %) disagree with what they
    // teach, and that set is *every profession-learn row* — spell 2020 is "Apprentice Blacksmith"
    // with no subtext where the taught 2018 is "Blacksmithing"/"Apprentice". Pet rows agree on both
    // columns, which is how the wrong hop survived this long.
    // `wire.spell` (the wrapper) stays the buy id below (CMSG_TRAINER_BUY_SPELL names it); a spell
    // that teaches nothing (a plain ability, or before Spell.dbc loads) resolves to itself.
    let taught = spells.learned_spell(wire.spell).unwrap_or(wire.spell);
    let display = spells.get(wire.spell);
    let cat = category(wire.state);
    // The SKILL gate has no per-gate "met" bit on the 5875 wire, so approximate it from the service's
    // overall category (an unavailable service has some unmet gate). The real client computes the
    // skill gate locally too — player skill value ≥ required, like the level gate the XML already
    // checks with `UnitLevel` — so a faithful per-gate skill check waits only on threading the
    // player's skill values here; the ability gate below already does its own per-gate check, so a
    // skill-gated service is the one remaining coarse case (decision 0253).
    let skill_met = cat != TrainerServiceCategory::Unavailable;
    let skill_req = (wire.req_skill != 0).then(|| TrainerSkillReq {
        name: skill_lines
            .and_then(|c| c.line(wire.req_skill))
            .map(|l| l.name.clone())
            .unwrap_or_else(|| format!("Skill {}", wire.req_skill)),
        rank: wire.req_skill_value,
        met: skill_met,
    });
    // Each prerequisite ability is coloured by whether the player already KNOWS that specific spell —
    // the real-client mechanism (`0x4d96e0`):
    // `GetTrainerServiceAbilityReq`'s hasReq is `IsSpellKnown(reqSpellId)`, evaluated per-requirement
    // and INDEPENDENT of the service's overall category — so a spell gated only by LEVEL still shows
    // its already-learned prev-rank prerequisite WHITE, not red. The req id is a real ability id (not
    // a learn wrapper), so there's no hop: look it up directly. The name carries
    // its rank exactly as the client does — `SpellDisplay::ranked_name`, the shared composer for the
    // client's `"%s (%s)"` literal (decision 2243). The client also ORs `KnownHigherRank`; benilla
    // has no rank chain, and sequential trainer ranks never reach that clause, so the direct
    // known-check covers every real case.
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
    // The tree's grouping key — its own byte-verified law, and per trainer type ([`service_group`],
    // decision 1124). Only the skill-line arm (types 0/1/3) can fail: the wire wrapper id itself is
    // never in `SkillLineAbility`, so that arm MUST go through the taught-spell hop above, and an
    // unresolved `0` drops the service from the tree exactly as the client's builder does. Log the
    // genuine miss (catalog present but no line) so DBC gaps surface rather than silently swallowing
    // a service the server offered. The tradeskill arm resolves no line at all and cannot miss.
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
        // The icon is its own byte-verified law over the WIRE spell ([`service_icon`]) — as, since
        // 1124, are the name and subtext above. Only the GROUP key still hops to the taught spell,
        // and only because the wrapper is not in `SkillLineAbility` at all.
        texture: service_icon(wire.spell, trainer_type, spells, icons, items, commands),
        // The detail pane's description body, left empty by design. The real
        // `GetTrainerServiceDescription` returns the spell's *tooltip* — `Spell.dbc`'s Description
        // with its `$s1`/`$o1`/`$d`/`$a1` tokens substituted from the spell's effect base points,
        // duration, and radius. Feeding the raw column would render the unsubstituted tokens
        // (broken-looking, not faithful), so descriptions wait on a spell-tooltip token engine —
        // its own arc, shared with the spellbook's hover tooltips — not a lone `SpellCatalog` column.
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

/// Build the Lua-facing snapshot from [`TrainerOpen`] + the spell/skill catalogs — `None` when no
/// trainer is open.
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
        // The engine synthesizes the tree in `set_trainer` — the app pushes only the flat services
        // (each carrying its resolved group key above).
        groups: Vec::new(),
    })
}

/// **The re-evaluator's inputs and three of its triggers** (2333), bundled: the player's own
/// descriptor (level, the skill slots) and its per-field edges (money, level, a skill slot —
/// decision 2297), the pet bar, and the object index + stores that resolve the pet's descriptor.
#[derive(SystemParam)]
struct ReEvalInputs<'w, 's> {
    self_player: Query<'w, 's, (Entity, &'static ObjectStore), With<SelfPlayer>>,
    stores: Query<'w, 's, &'static ObjectStore>,
    index: Res<'w, GuidIndex>,
    pet_bar: Res<'w, crate::ui_pet::PetBar>,
    edges: MessageReader<'w, 's, FieldChanged>,
}

/// Push the current trainer into the VM and fire the show/update/close events on a transition (or a
/// content change). Diffed against a `Local` memory, exactly like the gossip/merchant feeds. A
/// different trainer while the window is already open is a real close+open (the client's `ShowUIPanel`
/// early-returns when visible, so the open sound only re-plays after a hide — decision 0096).
#[allow(clippy::too_many_arguments)] // one Bevy system's full input set
fn feed_trainer(
    script: Option<NonSendMut<UiScript>>,
    // ResMut to consume the fresh-packet latch and to write the re-derived states (2333) — the
    // states are the one piece of trainer content the client authors, exactly as the reference's
    // `0x4d7d40` overwrites its own records.
    mut open: ResMut<TrainerOpen>,
    actions: Res<PlayerActions>,
    spells: Option<Res<Spells>>,
    skill_lines: Option<Res<SkillLines>>,
    mut re_eval: ReEvalInputs,
    // A tradeskill trainer's rows front the CREATED ITEM's icon, so the feed needs the ask-once
    // template cache + `ItemDisplayInfo.dbc` — the tradeskill window's own pair ([`service_icon`]).
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
    // Refusals go to the log and nowhere else — the reference's own console-only handling
    // ([`train_error_log_line`]). Logged rather than dropped for the reason 0651 logs the server's
    // dot-command answers: a refusal nothing said and a packet nothing noticed look identical
    // afterwards.
    for code in errors.0.drain(..) {
        info!("ui_trainer: {} (code {code})", train_error_log_line(code));
    }
    // Nothing to resolve a name/icon from yet — try again once Spell.dbc lands.
    let Some(spells) = spells.as_deref() else {
        return;
    };
    // The tree groups by skill line (decision 0247), so the skill-line catalog is required, not
    // optional: without it every service resolves to skill_line 0 and drops. A trainer only opens
    // well after world-entry, by when both DBCs have loaded, so this gate never actually delays a
    // real window — it just refuses to render an all-dropped empty tree.
    let Some(skill_lines) = skill_lines.as_deref() else {
        return;
    };
    // A new list packet resets the state filter and the collapse set in the engine, exactly as the
    // reference's builder does (decision 1128) — before the snapshot goes in, so `TRAINER_SHOW`
    // finds the reset mask and the window's own show handler pushes the SAVED filter back over it.
    if open.fresh_list {
        script.reset_trainer_list_state(open.trainer_type);
        open.fresh_list = false;
    }
    // The descriptor-side triggers of the re-evaluator (2333): money, level, any skill slot, on
    // the player's own object. Read every frame so the reader never backs up; they only matter
    // while a window is open.
    let self_player = re_eval.self_player.iter().next();
    for edge in re_eval.edges.read() {
        let Some((me, _)) = self_player else {
            continue;
        };
        if edge.entity != me {
            continue;
        }
        // The skill watchers (2336): the reference registers the VALUE word and the MAX and
        // PERMBONUS words of every slot (`0x5de180`, `0x5de450`), not the id/step dword and not
        // the temp bonus — so a slot's second dword always triggers and its third only when the
        // high half (permBonus) moved.
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
    // The re-evaluation itself — `0x4d7d40` over every row, then `TRAINER_UPDATE` regardless of
    // whether a state moved (its tail fires `0x136` unconditionally).
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
                            // The permanent bonus rides only a non-zero value (`0x4d8087`'s
                            // guard, the skill-pair reader's too).
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
    // The trainer's name resolves through the NameCache (a creature-name query, ask-once — the
    // real client's `UnitName("npc")`). `None`/empty while in flight; the title shows the static
    // "Trainer" until it lands, then re-fires TRAINER_UPDATE with the name as arg1 (the diff below
    // tracks the name too, so a name-only change still repaints the title). It rides an event arg
    // rather than a `TrainerState` field so no benilla-ui engine change is needed — the merchant
    // vendor-name pattern.
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
        // Close the old trainer, open the new: the frame hides then shows. The TRAINER_CLOSED routes
        // through the window's OnHide → CloseTrainer, which queues a close intent — consume it here so
        // the drain does NOT clear the trainer we just re-opened to (the merchant switch pattern).
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

/// The trainer window is an NPC session: the standardized range guard ([`crate::ui_session`])
/// client-side-closes it — the exact `CloseTrainer` clear — when the player walks out of the
/// trainer's service range or the trainer despawns.
impl NpcSession for TrainerOpen {
    fn npc(&self) -> Option<u64> {
        self.trainer
    }

    fn close(&mut self) {
        self.clear();
    }
}

/// Drain the Lua intents: the Train button's buys (`BuyTrainerService` queues the chosen row's spell
/// id) → `CMSG_TRAINER_BUY_SPELL` for the open trainer; a close → a local clear (no packet, vanilla).
/// The server answers a buy with `SMSG_TRAINER_BUY_SUCCEEDED` (→ the spell lands via
/// `SMSG_LEARNED_SPELL`, and the apply re-requests the list to repaint the row gray) or
/// `SMSG_TRAINER_BUY_FAILED` (→ the window's error line via [`TrainerErrors`]).
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
