//! The spell tooltip content, in the line order of the spell builder `0x52e610`: name and rank;
//! cost and range; cast time and cooldown (omitted for a passive spell, with no "Passive" line);
//! the tools, the required item class, then form, white when met and red when not; reagents, a
//! missing one inline red; the cooldown remaining; the chance and drain-all lines; the
//! description, gold. The aura builder `0x52f880` is [`super::tooltip_aura`]'s.
//!
//! The app resolves each spell into a [`SpellTooltipView`] (`$`-tokens, cast, range and duration
//! data, each `GlobalStrings.lua` template by key, read at the render), kept in an ask-once store
//! by spell id and pushed ahead of a hover: one store for the
//! views built against the player and one for those built against its pet, which the builder's
//! unit selector asks for. A miss records the id; a `set_spell_by_id` miss also waits, and the
//! app's answer re-renders that tooltip a frame after the hover. That frame is a known difference
//! from the reference, which builds every spell tooltip at the call (`0x52e610`); a spell no
//! pushed set names, such as an `enchant:` link's, always meets it. A talent miss shows its view on
//! the next hover. An aura view's miss waits the same way.

use mlua::{Lua, Table, Value};

use super::binding_abi::{self, flag};
use super::object::{frame_handle_of, frame_wrapper};
use super::tooltip::{append_line, clear, clear_silent, tip_mut};
use super::tooltip_aura::AuraKey;
use super::{CraftTooltip, Model, TooltipText, TrainerTooltip};
use crate::strings::Arg;
use crate::widget::FrameHandle;

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
/// The rank column's gray, `0xff808080`.
const GRAY: [f32; 4] = [128.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0, 1.0];
/// The description gold, `0xffffd200`.
const GOLD: [f32; 4] = [1.0, 210.0 / 255.0, 0.0, 1.0];

/// One spell's tooltip, prepared by the app: the data resolved, each `GlobalStrings.lua` template
/// named by key and read at the render ([`TooltipText`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpellTooltipView {
    pub name: String,
    /// "Rank N", the name line's gray right column.
    pub rank: Option<String>,
    /// The cost cell (`0x52e8f9`-`0x52e99f`): `MANA_COST` and its kin by power type (the key
    /// table `0x85416c`), `HEALTH_COST` past them, the `_PER_TIME` form with a cost per second.
    pub cost: Option<TooltipText>,
    /// `SPELL_RANGE` of the yards (`0x52ea4c`).
    pub range: Option<TooltipText>,
    /// The cast cell (`SPELL_CAST_TIME_*`, `SPELL_ON_NEXT_*`, `SPELL_CAST_CHANNELED`); `None` is a
    /// passive spell, whose cast and cooldown line is omitted.
    pub cast_time: Option<TooltipText>,
    /// The cooldown cell (`SPELL_RECAST_TIME_SEC 0x854ed4`): `max(RecoveryTime,
    /// CategoryRecoveryTime)`, as Charge's 15 s is in the category column.
    pub cooldown: Option<TooltipText>,
    /// The same larger recovery column in milliseconds (the builder's `[ebp-0x38]`, `0x52eb0d`):
    /// the live "Cooldown remaining" line and the setters' return flag need it at 60 000 or more.
    pub recovery_ms: u32,
    /// `SPELL_TOTEMS` (`0x854ec4`) and the tools of `Totem` (`+0xa0`), each the player does not
    /// carry wrapped in the inline `|cffff2020` escape (`0x52ed43`-`0x52eea2`).
    pub totems: Option<TooltipText>,
    /// The player carries every tool, which leaves the line out of a compact build.
    pub totems_met: bool,
    /// The required item's name, "Wands", that `SPELL_EQUIPPED_ITEM` (`0x854e94`) takes, from
    /// `EquippedItemClass` and `EquippedItemSubClassMask`.
    pub requires_item: Option<String>,
    /// A worn item matches the class and subclass mask; re-pushed on an equipment change.
    pub item_met: bool,
    /// The allowed forms' names joined, "Battle Stance", that `SPELL_REQUIRED_FORM` (`0x854e64`)
    /// takes, from the `Stances` mask.
    pub requires_form: Option<String>,
    /// The current shapeshift form matches the mask; the app re-pushes on a form change.
    pub form_met: bool,
    /// The required reputation, from `MinFactionId` and `MinReputation`.
    pub requires_reputation: Option<ReputationRequirement>,
    /// `SPELL_REAGENTS` (`0x854e54`) and the reagents; the app writes a missing reagent's inline
    /// `|cffff2020` escape.
    pub reagents: Option<TooltipText>,
    /// The player carries every reagent, which leaves the line out of a compact build.
    pub reagents_met: bool,
    /// `CHANCE_TO_DODGE` (`0x854e44`) and its siblings, by `Effect[0]`, of the player's live stat,
    /// which the app re-pushes as it moves.
    pub chance: Option<TooltipText>,
    /// The drain-all line of a spell with `AttributesEx` bit `0x2`: `SPELL_USE_ALL_MANA` and its
    /// kin by power type (the key table `0x854180`), `SPELL_USE_ALL_HEALTH` past them (`0x854dfc`),
    /// copied (`0x52f697`-`0x52f6e3`).
    pub use_all: Option<TooltipText>,
    /// The `$`-substituted description.
    pub description: TooltipText,
}

/// The spell's reputation line (`0x52f2ae`-`0x52f38e`): `ITEM_REQ_REPUTATION` (`0x854b8c`) of the
/// faction's name and the required rank's label, read off the active player whatever the caster.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReputationRequirement {
    /// The `Faction.dbc` row's name, `UNKNOWN` without one (`0x52f336`).
    pub faction: String,
    /// The rank, 0 Hated to 7 Exalted, whose label is `FACTION_STANDING_LABEL<rank + 1>`
    /// (`0x84b5dc`).
    pub rank: u32,
    /// The player's standing with the faction (`0x4d6370`) reaches the rank's floor (`0x52f2d3`).
    pub met: bool,
    /// The active player is female, so the label is the `_FEMALE` text when it is not empty
    /// (`0x612bf0`).
    pub female: bool,
}

impl super::UiScript {
    /// Store or replace a spell's view built against the player, answering its ask: a tooltip
    /// waiting on it re-renders now, a frame after its hover, where the reference builds it whole
    /// at the call (`0x52e610`).
    pub fn set_spell_tooltip(&mut self, spell_id: u32, view: SpellTooltipView) {
        self.store_spell_view(false, spell_id, view);
    }

    /// Store or replace a spell's view built against the player's pet, the one the builder's unit
    /// selector renders (`SetPetAction`, `SetSpell` on the pet book, a pet-learn trainer service).
    pub fn set_pet_spell_tooltip(&mut self, spell_id: u32, view: SpellTooltipView) {
        self.store_spell_view(true, spell_id, view);
    }

    fn store_spell_view(&mut self, alt_caster: bool, spell_id: u32, view: SpellTooltipView) {
        let waiting: Vec<FrameHandle> = {
            let mut model = self.model_mut();
            let (views, asks) = spell_store(&mut model, alt_caster);
            asks.remove(&spell_id);
            views.insert(spell_id, view);
            model
                .spell_tooltip_waits
                .iter()
                .filter(|(_, w)| w.subject.is_spell(spell_id, alt_caster))
                .map(|(h, _)| *h)
                .collect()
        };
        for h in waiting {
            if let Err(e) = answer_wait(&self.lua, h, &|w| w.is_spell(spell_id, alt_caster)) {
                self.push_error(e);
            }
        }
    }

    /// Drain the spell ids the renderers asked the player's store for and it lacked.
    pub fn take_spell_tooltip_asks(&mut self) -> Vec<u32> {
        self.model_mut().spell_tooltip_asks.drain().collect()
    }

    /// Drain the spell ids the renderers asked the pet's store for and it lacked.
    pub fn take_pet_spell_tooltip_asks(&mut self) -> Vec<u32> {
        self.model_mut().pet_spell_tooltip_asks.drain().collect()
    }

    /// The spells whose pet view a hover reads from the VM's own state: the pet's bar and book,
    /// whose setters pass the builder's unit selector (`0x532888`, `0x532e23`). The app pushes
    /// their views ahead of a hover.
    pub fn pet_spell_tooltip_subjects(&self) -> Vec<u32> {
        let model = self.model_ref();
        let pet_bar = model
            .pet_bar
            .slots
            .iter()
            .filter(|s| !s.view.is_token)
            .filter_map(|s| s.view.spell_id);
        let pet_book = model.pet_book.slots.iter().map(|s| s.spell_id);
        pet_bar.chain(pet_book).filter(|&id| id != 0).collect()
    }

    /// The spells whose player view a hover reads from the VM's own state rather than the
    /// player's book: the open quest's and each log entry's reward spell and the open craft's
    /// spell subjects. The app pushes their views ahead of a hover; the auras' views are
    /// [`Self::aura_tooltip_subjects`].
    pub fn spell_tooltip_subjects(&self) -> Vec<u32> {
        let model = self.model_ref();
        let rewards = model
            .quest
            .iter()
            .filter_map(|q| q.reward_spell.as_ref())
            .chain(
                model
                    .quest_log
                    .entries
                    .iter()
                    .filter(|e| !e.is_header)
                    .filter_map(|e| {
                        model
                            .quest_store
                            .records
                            .get(&e.quest_id)?
                            .reward_spell
                            .as_ref()
                    }),
            )
            .map(|s| s.spell_id);
        let craft = model
            .craft
            .iter()
            .flat_map(|c| &c.recipes)
            .filter_map(|r| match r.tooltip {
                CraftTooltip::Spell(id) => Some(id),
                CraftTooltip::Item(_) => None,
            });
        rewards.chain(craft).filter(|&id| id != 0).collect()
    }
}

/// A spell or aura render that missed its view, kept to re-run when the view lands. The re-run is
/// skipped unless the tooltip still shows `drawn`, so a line Lua added or rewrote since is kept.
#[derive(Clone)]
pub(crate) struct ViewWait {
    pub(super) subject: WaitFor,
    fallback_name: Option<String>,
    drawn: Drawn,
}

/// The view a render waits on and how it was called.
#[derive(Clone, PartialEq)]
pub(crate) enum WaitFor {
    Spell {
        spell_id: u32,
        opts: SpellRenderOpts,
    },
    /// The aura builder's key and `SetPlayerBuff`'s time line.
    Aura {
        key: AuraKey,
        remaining: Option<String>,
    },
}

impl WaitFor {
    fn is_spell(&self, id: u32, alt_caster: bool) -> bool {
        match self {
            WaitFor::Spell { spell_id, opts } => *spell_id == id && opts.alt_caster == alt_caster,
            WaitFor::Aura { .. } => false,
        }
    }
}

/// Each shown line's left and right cell as `(text, colour)`.
type Drawn = Vec<(Option<String>, Option<crate::color::Rgba8>)>;

fn drawn_lines(model: &mut Model, h: FrameHandle) -> mlua::Result<Drawn> {
    let t = tip_mut(model, h)?;
    let n = t.num_lines;
    let cells: Vec<_> = t
        .left_lines
        .iter()
        .take(n)
        .chain(t.right_lines.iter().take(n))
        .copied()
        .collect();
    Ok(cells
        .into_iter()
        .map(|rh| {
            let d = model.region_data.get(&rh);
            (
                d.and_then(|d| d.text.clone()),
                d.and_then(|d| d.vertex_color),
            )
        })
        .collect())
}

/// Record that `h`'s render missed the view `subject` names, with what it has drawn meanwhile.
/// Spell id 0 asks for nothing and waits on nothing.
pub(super) fn insert_wait(
    lua: &Lua,
    h: FrameHandle,
    subject: WaitFor,
    fallback_name: Option<String>,
) -> mlua::Result<()> {
    let id = match &subject {
        WaitFor::Spell { spell_id, .. } => *spell_id,
        WaitFor::Aura { key, .. } => key.spell_id,
    };
    if id == 0 {
        return Ok(());
    }
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let drawn = drawn_lines(&mut model, h)?;
    model.spell_tooltip_waits.insert(
        h,
        ViewWait {
            subject,
            fallback_name,
            drawn,
        },
    );
    Ok(())
}

/// Re-run the render `h` still waits with, if its subject is one `answers` names, against the
/// store read afresh: an earlier re-render's Lua may have replaced the wait. Lines Lua added or
/// rewrote since, or a fade under way, keep the tooltip as it is. The clear is silent: Lua made
/// one setter call and saw its one `OnTooltipCleared`.
pub(super) fn answer_wait(
    lua: &Lua,
    h: FrameHandle,
    answers: &dyn Fn(&WaitFor) -> bool,
) -> mlua::Result<()> {
    let (id, wait) = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        let Some(wait) = model
            .spell_tooltip_waits
            .get(&h)
            .filter(|w| answers(&w.subject))
            .cloned()
        else {
            return Ok(());
        };
        let fading = tip_mut(&mut model, h)?.fade_left.is_some();
        if fading || drawn_lines(&mut model, h)? != wait.drawn {
            model.spell_tooltip_waits.remove(&h);
            return Ok(());
        }
        (model.frame_id(h), wait)
    };
    clear_silent(lua, h);
    let this = frame_wrapper(lua, id)?;
    match wait.subject {
        WaitFor::Spell { spell_id, opts } => {
            fill_spell(lua, &this, h, spell_id, wait.fallback_name, opts)?;
        }
        WaitFor::Aura { key, remaining } => {
            super::tooltip_aura::fill_aura(lua, &this, h, key, wait.fallback_name, remaining)?;
        }
    }
    // As the reference's spell callback sizes after its builder's show (`0x52f869`).
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    super::tooltip::size_tooltip(&mut model, h);
    Ok(())
}

/// The view store and its asks for the unit a render is built against: the pet's when the
/// builder's unit selector is set (`alt_caster`), else the player's.
fn spell_store(
    model: &mut Model,
    alt_caster: bool,
) -> (
    &mut std::collections::HashMap<u32, SpellTooltipView>,
    &mut std::collections::HashSet<u32>,
) {
    if alt_caster {
        (
            &mut model.pet_spell_tooltips,
            &mut model.pet_spell_tooltip_asks,
        )
    } else {
        (&mut model.spell_tooltips, &mut model.spell_tooltip_asks)
    }
}

/// Look up a spell's view built against the player, or with `alt_caster` against its pet; a miss
/// records the ask.
pub(super) fn spell_view_of(
    lua: &Lua,
    spell_id: u32,
    alt_caster: bool,
) -> Option<SpellTooltipView> {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let (views, asks) = spell_store(&mut model, alt_caster);
    let v = views.get(&spell_id).cloned();
    if v.is_none() && spell_id != 0 {
        asks.insert(spell_id);
    }
    v
}

/// The talent lines [`render_spell`] interleaves (`TOOLTIP_TALENT_RANK 0x854a2c`,
/// `TOOLTIP_TALENT_LEARN 0x8549f8`): the white "Rank r/m" after the name, the red requirements
/// while locked, the next-rank block and the green learn hint.
#[derive(Clone, Debug, Default)]
pub(super) struct TalentLines {
    /// `Talent.dbc` Flags bit 0: `SetTalent` hands an exceptional talent to the spell builder
    /// `0x52e610` (`0x5352ab`) and any other to the talent builder `0x52b0a0`, which writes no
    /// cost, range, cast, required-item, form, reagent or chance line.
    pub exceptional: bool,
    /// `TOOLTIP_TALENT_RANK` ("Rank %d/%d") filled from the player's strings; `None` when they
    /// lack the key, and no rank row shows.
    pub rank_line: Option<String>,
    pub reqs: Vec<String>,
    /// The next rank's spell id, 0 for none, asked from the store while `next_desc` is missing.
    pub next_spell: u32,
    pub next_desc: Option<TooltipText>,
    pub learn: bool,
}

/// The talent learn hint's green, `0xff00ff00`.
const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
/// The unmet-requirement red, `0xffff2020` (`0xc0d390`, the item builder's red).
const RED: [f32; 4] = [1.0, 32.0 / 255.0, 32.0 / 255.0, 1.0];

/// The spell builder's options (`0x52e610`'s stack arguments after the spell id, `p2` to `p5`);
/// `Default` is the plain spell hover.
#[derive(Clone, Copy, Default, PartialEq)]
pub(crate) struct SpellRenderOpts {
    /// `p2` compact, which `SetAction` and `SetShapeshift` pass when the `UberTooltips` option is
    /// 0 or missing ([`compact_tooltips`]): the rank shows (`0x52e7fe`), the cost, range and cast
    /// lines drop (`0x52e8b4`, `0x52eb15`), so do met totem, item, form and reagent lines, an unmet
    /// item or form takes its `_NOSPACE` key, and the build ends after the cooldown remaining
    /// (`0x52f589`), with no chance, use-all or description line.
    pub(super) compact: bool,
    /// `p5` showRank, the gray "Rank N" column (`0x52e809`): `SetAction`, `SetShapeshift` and
    /// `SetPetAction` pass 1, every other setter 0.
    pub(super) show_rank: bool,
    /// `p4` altCaster, the unit selector: the view is the one built against the player's charm,
    /// else its summon (`0x6e3130`, `0x6e31b0`, `0x6e3340`), the totem and reagent lines drop
    /// (`0x52ed50`, `0x52f398`) and the item requirement counts as met (`0x52f07f`).
    /// `SetPetAction` sets it (`0x532888`), `SetSpell` on the pet book (`0x532e23`) and
    /// `SetTrainerService` for a `LEARN_PET_SPELL` service (`0x533a7f`).
    pub(super) alt_caster: bool,
    /// `p3`, the live cooldown left in ms (`start + duration - now`), 0 for none: the setter takes
    /// it from the cooldown query (`0x6e2ea0`) and passes it only while that cooldown runs
    /// (enabled, with a start and a duration), as `0x532e55`..`0x532e81` does.
    pub(super) remaining_ms: u32,
}

/// `SetAction` and `SetShapeshift`'s `p2`: the `UberTooltips` option looked up by name
/// (`0x63de30`, `0x532622`, `0x532a35`), compact when it is missing or its integer is 0
/// (`0x532627`-`0x532632`).
fn compact_tooltips(lua: &Lua) -> bool {
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    model
        .cvars
        .get("ubertooltips")
        .is_none_or(|s| binding_abi::s_str_to_int(&s.value) == 0)
}

/// The live cooldown left in ms for the builder's `p3`, from a `(start, duration, enabled)`
/// triple in `GetTime` seconds: nonzero only while the cooldown runs, enabled with a start and a
/// duration (`0x532e5a`..`0x532e81`, `0x5328aa`), else 0. An item action's block takes the same
/// number (`0x5324d5`).
pub(super) fn live_remaining_ms(cooldown: Option<(f64, f64, bool)>, now: f64) -> u32 {
    let Some((start, duration, true)) = cooldown else {
        return 0;
    };
    if start <= 0.0 || duration <= 0.0 {
        return 0;
    }
    ((start + duration - now) * 1000.0)
        .round()
        .clamp(0.0, f64::from(u32::MAX)) as u32
}

/// Whether the spell builder draws ITEM_COOLDOWN_TIME and sets its return value: `p3` nonzero
/// and a recovery column of a minute or more (`0x52f537`..`0x52f581`). A shorter cooldown shows
/// only in the recast cell.
fn cooldown_countdown(v: &SpellTooltipView, opts: SpellRenderOpts) -> bool {
    opts.remaining_ms > 0 && v.recovery_ms >= 60_000
}

/// Renders the spell and answers the builder's return value (`[ebp-0x28]`): whether the tooltip
/// holds a line that changes while it is shown, the chance line (`0x52f68c`) or the remaining
/// cooldown (`0x52f581`). The setters push it as 1 or nil, and the stock buttons arm
/// `updateTooltip` from it.
fn render_spell(
    lua: &Lua,
    this: &Table,
    v: &SpellTooltipView,
    opts: SpellRenderOpts,
    talent: Option<&TalentLines>,
) -> mlua::Result<bool> {
    let SpellRenderOpts {
        compact,
        show_rank,
        alt_caster,
        ..
    } = opts;
    let mut live = false;
    // The name white (`0x530270`), the rank gray at right when compact or asked for (`0x52e7fe`,
    // `0x52e809`).
    let right = v
        .rank
        .clone()
        .filter(|_| compact || show_rank)
        .map(|t| (t, GRAY));
    append_line(lua, this, (v.name.clone(), WHITE), right, false)?;
    let talent_builder = talent.is_some_and(|t| !t.exceptional);
    // The talent head: the white rank line, then the red requirements while locked.
    if let Some(t) = talent {
        if let Some(rank) = &t.rank_line {
            append_line(lua, this, (rank.clone(), WHITE), None, false)?;
        }
        for req in &t.reqs {
            append_line(lua, this, (req.clone(), RED), None, true)?;
        }
    }
    // Each cell's templates, read now (`0x703bf0` at the call); an empty one is no cell.
    let cell = |t: &Option<TooltipText>| t.as_ref().and_then(|t| t.line(lua));
    // A compact build has no cost, range or cast line (`0x52e8b4`, `0x52eb15`).
    if !talent_builder && !compact {
        match (cell(&v.cost), cell(&v.range)) {
            (Some(c), Some(r)) => append_line(lua, this, (c, WHITE), Some((r, WHITE)), false)?,
            // With no cost the range takes the left cell (`0x52ea94`-`0x52eaae`).
            (Some(c), None) | (None, Some(c)) => append_line(lua, this, (c, WHITE), None, false)?,
            (None, None) => {}
        }
        // The cast line, only for a spell with a cast cell (`0x52eb15`-`0x52eb3f`); the recast
        // cell alone is no line.
        if v.cast_time.is_some() {
            match (cell(&v.cast_time), cell(&v.cooldown)) {
                (Some(ct), cd) => {
                    append_line(lua, this, (ct, WHITE), cd.map(|cd| (cd, WHITE)), false)?
                }
                // An empty cast cell keeps the recast cell at the right (`0x530270`).
                (None, Some(cd)) => {
                    append_line(lua, this, (String::new(), WHITE), Some((cd, WHITE)), false)?
                }
                (None, None) => {}
            }
        }
    }
    if !talent_builder {
        // The tools, white and unwrapped, unless altCaster drops them (`0x52ed50`) or a compact
        // build has them all (`0x52ee83`-`0x52ee8f`).
        if let Some(totems) = cell(&v.totems)
            .filter(|_| !alt_caster)
            .filter(|_| !(compact && v.totems_met))
        {
            append_line(lua, this, (totems, WHITE), None, false)?;
        }
        // The item and form requirements (`0x52f084`-`0x52f102`, `0x52f22b`-`0x52f2a6`): a
        // compact build leaves a met one out and words an unmet one with the `_NOSPACE` key. The
        // pet's item requirement counts as met (`0x52f07f`).
        let requirement = |key: &str, name: &str, met: bool| match (compact, met) {
            (true, true) => None,
            (true, false) => keyed_line(lua, &format!("{key}_NOSPACE"), name),
            (false, _) => keyed_line(lua, key, name),
        };
        if let Some(name) = &v.requires_item {
            let met = v.item_met || alt_caster;
            if let Some(text) = requirement("SPELL_EQUIPPED_ITEM", name, met) {
                let color = if met { WHITE } else { RED };
                append_line(lua, this, (text, color), None, true)?;
            }
        }
        if let Some(names) = &v.requires_form {
            if let Some(text) = requirement("SPELL_REQUIRED_FORM", names, v.form_met) {
                let color = if v.form_met { WHITE } else { RED };
                append_line(lua, this, (text, color), None, false)?;
            }
        }
        // The reputation requirement, white when met, else red, unwrapped; a compact build leaves
        // a met one out, and an unmet one keeps its key (`0x52f2df`-`0x52f38e`).
        if let Some(r) = v
            .requires_reputation
            .as_ref()
            .filter(|r| !(compact && r.met))
        {
            if let Some(text) = reputation_line(lua, r) {
                let color = if r.met { WHITE } else { RED };
                append_line(lua, this, (text, color), None, false)?;
            }
        }
        // Reagents, white and wrapped, unless altCaster drops them (`0x52f398`) or a compact
        // build has them all (`0x52f513`-`0x52f51f`).
        if let Some(reagents) = cell(&v.reagents)
            .filter(|_| !alt_caster)
            .filter(|_| !(compact && v.reagents_met))
        {
            append_line(lua, this, (reagents, WHITE), None, true)?;
        }
        // ITEM_COOLDOWN_TIME (`0x854adc`), white and unwrapped, after the reagents and before the
        // chance line (`0x52f537`..`0x52f577`), through `0x52fa50`'s round-up ladder.
        if cooldown_countdown(v, opts) {
            let line = super::tooltip::duration_text(
                opts.remaining_ms,
                "ITEM_COOLDOWN_TIME",
                true,
                &|key| crate::strings::global(lua, key),
            );
            if let Some(line) = line {
                append_line(lua, this, (line, WHITE), None, false)?;
            }
        }
        // A compact build ends here, shown (`0x52f589`-`0x52f5ae`).
        if compact {
            return Ok(cooldown_countdown(v, opts));
        }
        // The chance line, white and unwrapped, between the reagents and the description. Its
        // presence sets the return value (`0x52f68c`).
        if v.chance.is_some() {
            if let Some(chance) = cell(&v.chance) {
                append_line(lua, this, (chance, WHITE), None, false)?;
            }
            live = true;
        }
        // The drain-all line, white and unwrapped, before the description (`0x52f6e3`).
        if let Some(use_all) = cell(&v.use_all) {
            append_line(lua, this, (use_all, WHITE), None, false)?;
        }
    }
    let desc = &v.description;
    if let Some(text) = desc.line(lua) {
        append_line(lua, this, (text, GOLD), None, true)?;
    }
    // The talent tail: the white `TOOLTIP_TALENT_NEXT_RANK` header (`0x854a10`, pushed at
    // `0x52b2cd`) over the next rank's gold description, then the green `TOOLTIP_TALENT_LEARN`
    // hint (`0x52b362`). Both are the player's own strings; without them the line is skipped.
    if let Some(t) = talent {
        // The talent builder skips a rank with an empty description, header and all (`0x52b294`).
        let next = t
            .next_desc
            .as_ref()
            .filter(|n| !talent_builder || !n.is_empty());
        if let Some(next) = next {
            // The talent builder's gold `" "` spacer (`0x82ee00`, `0x52b2a8`) after a description.
            if talent_builder && !desc.is_empty() {
                append_line(lua, this, (" ".into(), GOLD), None, false)?;
            }
            if let Some(header) = crate::strings::global(lua, "TOOLTIP_TALENT_NEXT_RANK") {
                append_line(lua, this, (header, WHITE), None, false)?;
            }
            if let Some(next) = next.line(lua) {
                append_line(lua, this, (next, GOLD), None, true)?;
            }
        }
        if t.learn {
            if let Some(hint) = crate::strings::global(lua, "TOOLTIP_TALENT_LEARN") {
                append_line(lua, this, (hint, GREEN), None, false)?;
            }
        }
    }
    Ok(live || (!talent_builder && cooldown_countdown(v, opts)))
}

/// A requirement line: `key`'s template of `name`, read now; none when the key is missing.
/// `ITEM_REQ_REPUTATION` of the faction and the rank's gendered label
/// ([`crate::strings::standing_label`]).
fn reputation_line(lua: &Lua, r: &ReputationRequirement) -> Option<String> {
    let label =
        crate::strings::standing_label(&|k| crate::strings::global(lua, k), r.rank, r.female);
    let template = crate::strings::global(lua, "ITEM_REQ_REPUTATION")?;
    Some(crate::strings::fill(
        &template,
        &[Arg::S(&r.faction), Arg::S(&label)],
    ))
    .filter(|t| !t.is_empty())
}

fn keyed_line(lua: &Lua, key: &str, name: &str) -> Option<String> {
    let template = crate::strings::global(lua, key)?;
    Some(crate::strings::fill(&template, &[Arg::S(name)])).filter(|t| !t.is_empty())
}

/// `GameTooltip:SetTalent`'s render: the talent's spell plus its talent lines; a missing next-rank
/// view is asked for.
pub(super) fn set_spell_with_talent(
    lua: &Lua,
    this: &Table,
    spell_id: u32,
    talent: TalentLines,
) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    clear(lua, h);
    if talent.next_desc.is_none() {
        super::talent::ask_next_rank(lua, talent.next_spell);
    }
    match spell_view_of(lua, spell_id, false) {
        Some(v) => {
            render_spell(lua, this, &v, SpellRenderOpts::default(), Some(&talent))?;
        }
        None => {
            // No view yet: the rank line alone until the next hover.
            if let Some(rank) = &talent.rank_line {
                append_line(lua, this, (rank.clone(), WHITE), None, false)?;
            }
        }
    }
    super::tooltip::show_or_hide_empty(lua, h);
    Ok(())
}

/// The reward-spell hover: the spell, its name as the fallback, or an empty tooltip.
fn set_reward_spell(
    lua: &Lua,
    this: &Table,
    spell: Option<(u32, Option<String>)>,
) -> mlua::Result<()> {
    match spell {
        Some((id, name)) => set_spell_by_id(lua, this, id, name, SpellRenderOpts::default()),
        None => {
            let h = frame_handle_of(lua, this)?;
            clear_silent(lua, h);
            super::tooltip::show_or_hide_empty(lua, h);
            Ok(())
        }
    }
}

pub(super) fn set_spell_by_id(
    lua: &Lua,
    this: &Table,
    spell_id: u32,
    fallback_name: Option<String>,
    opts: SpellRenderOpts,
) -> mlua::Result<()> {
    set_spell_flagged(lua, this, spell_id, fallback_name, opts).map(|_| ())
}

/// [`set_spell_by_id`] answering the builder's return value (see [`render_spell`]). A view still
/// in flight answers false: the wait paints the tooltip when the app answers, and the setter has
/// returned by then.
pub(super) fn set_spell_flagged(
    lua: &Lua,
    this: &Table,
    spell_id: u32,
    fallback_name: Option<String>,
    opts: SpellRenderOpts,
) -> mlua::Result<bool> {
    let h = frame_handle_of(lua, this)?;
    clear(lua, h);
    fill_spell(lua, this, h, spell_id, fallback_name, opts)
}

/// Fill a cleared tooltip with the spell's view, or with the fallback name and a wait on the ask.
fn fill_spell(
    lua: &Lua,
    this: &Table,
    h: FrameHandle,
    spell_id: u32,
    fallback_name: Option<String>,
    opts: SpellRenderOpts,
) -> mlua::Result<bool> {
    let live = match spell_view_of(lua, spell_id, opts.alt_caster) {
        Some(v) => render_spell(lua, this, &v, opts, None)?,
        None => {
            if let Some(name) = fallback_name.clone() {
                append_line(lua, this, (name, WHITE), None, false)?;
            }
            // The fallback stands until the app answers the ask.
            insert_wait(lua, h, WaitFor::Spell { spell_id, opts }, fallback_name)?;
            false
        }
    };
    super::tooltip::show_or_hide_empty(lua, h);
    Ok(live)
}

/// Register the spell and aura content methods on the GameTooltip method table.
pub(super) fn install_methods(lua: &Lua, m: &Table) -> mlua::Result<()> {
    // GameTooltip:SetSpell(bookId, bookType): the spellbook hover. `SetSpell 0x532d10` bounds the
    // 1-based slot to `[0, 0x400)` (`0x532dd4`) and reads the pet book (`0xb6f098`) when `bookType`
    // is "pet" (`0x532e13`), else the player's (`0xb700f0`); the pet flag also picks the cooldown
    // bank it passes to `0x6e2ea0` (`0x532e50`).
    m.set(
        "SetSpell",
        lua.create_function(|lua, (this, book_id, book_type): (Table, Value, Value)| {
            // The shared slot marshaller's gates (`0x532db3`, `0x532dc0`, `0x532dd9`..`0x532df4`):
            // a number index and a number or string `bookType` (a number is stringified and so is
            // the player's book), the index `trunc(n - 1)` in `[0, 0x400)`, else the binding
            // raises "Invalid spell slot in SetSpell" (`0x532ec8`, string `0x855290`).
            let (book_id, book_type) =
                super::spellbook::spell_slot_args(book_id, book_type, "SetSpell")?;
            let now = crate::script::clock::now(lua);
            let (spell_id, name, remaining_ms) = {
                let model = lua.app_data_mut::<Model>().expect("model app_data");
                match super::spellbook::book_slot(&model, book_id, &book_type) {
                    Some(s) => {
                        let cooldown = s.cooldown.map(|(start_ms, duration_ms, enabled)| {
                            (
                                start_ms as f64 / 1000.0,
                                f64::from(duration_ms) / 1000.0,
                                enabled,
                            )
                        });
                        (
                            s.spell_id,
                            Some(s.name.clone()),
                            live_remaining_ms(cooldown, now),
                        )
                    }
                    None => return Ok(Value::Nil),
                }
            };
            // The pet book's flag is also the builder's unit selector (`0x532e70`).
            let opts = SpellRenderOpts {
                alt_caster: super::spellbook::is_pet_book(&book_type),
                remaining_ms,
                ..Default::default()
            };
            Ok(flag(set_spell_flagged(lua, &this, spell_id, name, opts)?))
        })?,
    )?;
    // GameTooltip:SetShapeshift(index): the stance-bar hover, the form's spell tooltip. It answers
    // the builder's return value, 1 or nil, with a cooldown running (`0x532a89`).
    m.set(
        "SetShapeshift",
        lua.create_function(|lua, (this, index): (Table, Value)| {
            // `is-number` else `Usage: SetShapeshift(slot)` (`0x85523c`, `0x532ad6`).
            let index = binding_abi::index_arg(lua, index, "Usage: SetShapeshift(slot)")?;
            let now = crate::script::clock::now(lua);
            let (spell_id, name, remaining_ms) = {
                let model = lua.app_data_mut::<Model>().expect("model app_data");
                match model.shapeshift_forms.get(index.saturating_sub(1)) {
                    Some(f) => (
                        f.view.spell_id,
                        Some(f.view.name.clone()),
                        live_remaining_ms(f.cooldown, now),
                    ),
                    None => return Ok(Value::Nil),
                }
            };
            // Both legs pass the rank and the `UberTooltips` compact flag (`0x532a62`-`0x532a84`,
            // `0x532aaf`-`0x532abe`).
            let opts = SpellRenderOpts {
                compact: compact_tooltips(lua),
                show_rank: true,
                remaining_ms,
                ..Default::default()
            };
            // The builder's value is read only on the running-cooldown leg (`0x532a89`); the
            // other leg builds with a zero remainder and pushes nil (`0x532abe`, `0x532ac3`).
            let live = set_spell_flagged(lua, &this, spell_id, name, opts)?;
            Ok(flag(remaining_ms > 0 && live))
        })?,
    )?;
    // GameTooltip:SetPetAction(index): the pet-bar hover for a spell slot, built against the pet
    // (the unit selector, pushed at `0x532888` and `0x5328d0`); the stock
    // `PetActionButton_OnEnter` builds a token's tooltip itself (`PetActionBarFrame.lua:290`). A
    // slot with no spell is a no-op answering nil; a spell slot answers the builder's return value,
    // 1 or nil, with a cooldown running (`0x5328ca`).
    m.set(
        "SetPetAction",
        lua.create_function(|lua, (this, index): (Table, Value)| {
            // `is-number` else `Usage: SetPetAction(slot)` (`0x855220`, `0x5328f4`).
            let index = binding_abi::index_arg(lua, index, "Usage: SetPetAction(slot)")?;
            let now = crate::script::clock::now(lua);
            let (spell_id, name, remaining_ms) = {
                let model = lua.app_data_mut::<Model>().expect("model app_data");
                match model
                    .pet_bar
                    .slots
                    .get(index.saturating_sub(1))
                    .filter(|s| !s.view.is_token)
                {
                    Some(s) => match s.view.spell_id {
                        Some(id) => (id, s.view.name.clone(), live_remaining_ms(s.cooldown, now)),
                        None => return Ok(Value::Nil),
                    },
                    None => return Ok(Value::Nil),
                }
            };
            // The unit selector and the rank on both legs (`0x532886`, `0x5328ce`).
            let opts = SpellRenderOpts {
                alt_caster: true,
                show_rank: true,
                remaining_ms,
                ..Default::default()
            };
            // As `SetShapeshift`: the builder's value counts only with a cooldown running
            // (`0x5328aa`; the other leg builds and pushes nil, `0x5328dc`, `0x5328e1`).
            let live = set_spell_flagged(lua, &this, spell_id, name, opts)?;
            Ok(flag(remaining_ms > 0 && live))
        })?,
    )?;
    // GameTooltip:SetQuestRewardSpell() / SetQuestLogRewardSpell() (`0x535bb0`, `0x535c60`): the
    // hover of a reward slot whose `rewardType` is "spell" (`QuestFrameTemplates.xml:150`,
    // `QuestLogFrame.xml:115`), the quest's reward spell or an empty tooltip.
    m.set(
        "SetQuestRewardSpell",
        lua.create_function(|lua, this: Table| {
            let id = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model
                    .quest
                    .as_ref()
                    .and_then(|q| q.reward_spell.as_ref())
                    .map(|s| (s.spell_id, s.name.clone()))
            };
            set_reward_spell(lua, &this, id)
        })?,
    )?;
    m.set(
        "SetQuestLogRewardSpell",
        lua.create_function(|lua, this: Table| {
            let id = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model
                    .selected_quest_record()
                    .and_then(|r| r.reward_spell.as_ref())
                    .map(|s| (s.spell_id, s.name.clone()))
            };
            set_reward_spell(lua, &this, id)
        })?,
    )?;
    // GameTooltip:SetAction(slot): the action-bar hover by payload kind (`0x5322a0`): a spell
    // (0x00) to the spell builder `0x52e610`, an item (0x80) to the item builder `0x52b650`, a
    // macro (0x40) to `0x52b040`, which fetches it (`0x4f0f40`) and renders only its name, white
    // (`0x5303b0` in colour `0xc0cf60`), or hides with none; 1.12 has no `#showtooltip`. It
    // answers the spell or item builder's return value as 1 or nil (`0x5325a9`), the spell's only
    // with a cooldown running, nil for the macro leg, an empty slot and any other kind;
    // `ActionButton_SetTooltip` arms `updateTooltip` from it.
    m.set(
        "SetAction",
        lua.create_function(|lua, (this, slot): (Table, Value)| {
            // `is-number` else `Usage: SetAction(slot)` (`0x855200`, `0x5326c3`).
            let slot = binding_abi::index_arg32(lua, slot, "Usage: SetAction(slot)")?;
            let now = crate::script::clock::now(lua);
            let (action, remaining_ms, owned) = {
                let model = lua.app_data_mut::<Model>().expect("model app_data");
                let action = model.actions.get(&slot).cloned();
                // An item word resolves to the player's own item by entry (`0x532490`).
                let owned = action
                    .as_ref()
                    .filter(|a| a.kind == 0x80)
                    .and_then(|a| super::tooltip_item::owned_instance(&model, a.action));
                (
                    action,
                    live_remaining_ms(model.action_states.get(&slot).and_then(|s| s.cooldown), now),
                    owned,
                )
            };
            let Some(a) = action else {
                return Ok(Value::Nil);
            };
            match a.kind {
                // The builder's value counts only with a cooldown running (`0x532597`); the other
                // leg builds with a zero remainder and pushes nil (`0x53267c`, `0x532690`), so
                // Attack, Parry, Block and Dodge answer nil here, whatever their chance line.
                0x00 => {
                    let live = set_spell_flagged(
                        lua,
                        &this,
                        a.action,
                        None,
                        SpellRenderOpts {
                            compact: compact_tooltips(lua),
                            show_rank: true,
                            remaining_ms,
                            ..Default::default()
                        },
                    )?;
                    Ok(flag(remaining_ms > 0 && live))
                }
                // The item builder's block takes the same live cooldown (`0x5324d5`), whose
                // ITEM_COOLDOWN_TIME line sets the return value (`0x52e140`). The tooltip is
                // built either way, but a miss in the player's own items pushes nil whatever the
                // builder returned (`0x5325bc`, `0x532604`, `0x532404`).
                0x80 => {
                    let mut owned = owned;
                    if let Some(inst) = owned.as_mut() {
                        inst.cooldown_ms = remaining_ms;
                    }
                    let found = owned.is_some();
                    let live = super::tooltip_item::render_owned(lua, &this, a.action, owned)?;
                    Ok(flag(found && live))
                }
                0x40 => {
                    let h = frame_handle_of(lua, &this)?;
                    clear(lua, h);
                    // The same 1..36 lookup as `GetActionText` and the bar icon, so the three
                    // agree on a slot's macro.
                    let name = {
                        let model = lua.app_data_ref::<Model>().expect("model app_data");
                        model.macros.get(a.action as usize).map(|m| m.name.clone())
                    };
                    if let Some(name) = name {
                        append_line(lua, &this, (name, WHITE), None, false)?;
                    }
                    super::tooltip::show_or_hide_empty(lua, h);
                    // The macro leg's builder return is dropped (`0x5326be`).
                    Ok(Value::Nil)
                }
                _ => Ok(Value::Nil),
            }
        })?,
    )?;

    // GameTooltip:SetTrainerService(index): the trainer detail icon's hover
    // (`Blizzard_TrainerUI.xml:452`); list rows have no tooltip. `0x5338b0` writes no line itself
    // and hands a shared builder the subject the app resolved (`TrainerService::tooltip`). `index`
    // is a visible row, headers included; a header is a no-op.
    m.set(
        "SetTrainerService",
        lua.create_function(|lua, (this, index): (Table, Value)| {
            // `is-number` else `Invalid trainer service in SetTrainerService(index)` (`0x8553a4`,
            // `0x533968`).
            let index = binding_abi::index_arg(
                lua,
                index,
                "Invalid trainer service in SetTrainerService(index)",
            )?;
            let subject = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                match super::trainer::service(&model, index) {
                    Some(s) => s.tooltip.clone(),
                    None => return Ok(()),
                }
            };
            match subject {
                // The item builder (`0x52b650`), with no fallback name: id 0 or a template in
                // flight renders empty, the builder's own early-out.
                TrainerTooltip::Item(item_id) => {
                    super::tooltip_item::render_by_id(lua, &this, item_id, None, None)
                }
                TrainerTooltip::Spell {
                    spell_id,
                    alt_caster,
                } => set_spell_by_id(
                    lua,
                    &this,
                    spell_id,
                    None,
                    SpellRenderOpts {
                        alt_caster,
                        ..Default::default()
                    },
                ),
            }
        })?,
    )?;
    // GameTooltip:SetCraftSpell(craftIndex): the craft detail icon's hover
    // (`Blizzard_CraftUI.xml:566`). `SetCraftSpell 0x533e90` hands a shared builder the subject the
    // app resolved from the recipe's own effects (`CraftRecipe::tooltip`). `craftIndex` is a plain
    // recipe position: the craft list has no headers.
    m.set(
        "SetCraftSpell",
        lua.create_function(|lua, (this, craft_index): (Table, Value)| {
            // `is-number` else `Invalid craft in SetCraftSpell(index)` (`0x85544c`, `0x533f54`).
            let craft_index =
                binding_abi::index_arg(lua, craft_index, "Invalid craft in SetCraftSpell(index)")?;
            let subject = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                let Some(c) = &model.craft else {
                    return Ok(());
                };
                match craft_index.checked_sub(1).and_then(|i| c.recipes.get(i)) {
                    Some(r) => r.tooltip.clone(),
                    None => return Ok(()),
                }
            };
            match subject {
                CraftTooltip::Item(item_id) => {
                    super::tooltip_item::render_by_id(lua, &this, item_id, None, None)
                }
                CraftTooltip::Spell(spell_id) => {
                    set_spell_by_id(lua, &this, spell_id, None, SpellRenderOpts::default())
                }
            }
        })?,
    )?;
    Ok(())
}
