//! The spell and aura tooltip content, in the line order of the spell builder `0x52e610` and the
//! aura builder `0x52f880`: name and rank; cost and range; cast time and cooldown (omitted for a
//! passive spell, with no "Passive" line); the required item class, then form, white when met and
//! red when not; reagents, a missing one inline red; the description, gold (white on an aura).
//! Only `SetPlayerBuff` adds the gold time-remaining line.
//!
//! The app resolves each spell into a [`SpellTooltipView`] (`$`-tokens, cast, range and duration
//! text), kept in an ask-once store by spell id: a miss records the id, the app pushes the view and
//! the next hover repaints.

use mlua::{Lua, Table, Value};

use super::object::frame_handle_of;
use super::tooltip::{append_line, clear_content, fire_cleared};
use super::{CraftTooltip, Model, TrainerTooltip};

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
/// The rank column's gray, `0xff808080`.
const GRAY: [f32; 4] = [128.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0, 1.0];
/// The description gold, `0xffffd200`.
const GOLD: [f32; 4] = [1.0, 210.0 / 255.0, 0.0, 1.0];

/// One spell's tooltip, every string resolved by the app.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpellTooltipView {
    pub name: String,
    /// "Rank N", the name line's gray right column on the spell variant.
    pub rank: Option<String>,
    /// The aura variant's gold right column (`0x52f8e5`): the `SpellDispelType.dbc` name, such as
    /// "Magic", when that row's `[+0x28]` flag is set, so Stealth has none.
    pub dispel_type: Option<String>,
    /// The cost cell, such as "35 Mana" or "11 Health, plus 5 per sec": the resolved cost through
    /// the power-type keys, falling back to health.
    pub cost: Option<String>,
    pub range: Option<String>,
    /// The cast cell ("1.5 sec cast", "Instant", "Next melee", "Channeled"); `None` is a passive
    /// spell, whose cast and cooldown line is omitted.
    pub cast_time: Option<String>,
    /// The cooldown cell (`SPELL_RECAST_TIME_SEC 0x854ed4`): `max(RecoveryTime,
    /// CategoryRecoveryTime)`, as Charge's 15 s is in the category column.
    pub cooldown: Option<String>,
    /// "Requires Wands" (`SPELL_EQUIPPED_ITEM 0x854e94`), from `EquippedItemClass` and
    /// `EquippedItemSubClassMask`.
    pub requires_item: Option<String>,
    /// A worn item matches the class and subclass mask; re-pushed on an equipment change.
    pub item_met: bool,
    /// "Requires Battle Stance" (`SPELL_REQUIRED_FORM 0x854e64`), from the `Stances` mask.
    pub requires_form: Option<String>,
    /// The current shapeshift form matches the mask; the app re-pushes on a form change.
    pub form_met: bool,
    /// "Reagents: Light Feather" (`SPELL_REAGENTS 0x854e54` and the eight reagent slots); the app
    /// writes a missing reagent's inline `|cffff2020` escape.
    pub reagents: Option<String>,
    /// "2.62% chance to dodge" (`CHANCE_TO_DODGE 0x854e44` and its siblings, by `Effect[0]`),
    /// from the player's live stat, which the app re-pushes as it moves.
    pub chance: Option<String>,
    /// The `$`-substituted description.
    pub description: String,
    /// The `$`-substituted `AuraDescription`, which the aura builder reads; `description` stands in
    /// when it is empty.
    pub aura_description: String,
}

impl super::UiScript {
    /// Store or replace a spell's view, answering its ask.
    pub fn set_spell_tooltip(&mut self, spell_id: u32, view: SpellTooltipView) {
        let mut model = self.model_mut();
        model.spell_tooltip_asks.remove(&spell_id);
        model.spell_tooltips.insert(spell_id, view);
    }

    /// Drain the spell ids the renderers asked for and the store lacked.
    pub fn take_spell_tooltip_asks(&mut self) -> Vec<u32> {
        self.model_mut().spell_tooltip_asks.drain().collect()
    }
}

/// Look up a spell's view; a miss records the ask.
pub(super) fn spell_view_of(lua: &Lua, spell_id: u32) -> Option<SpellTooltipView> {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let v = model.spell_tooltips.get(&spell_id).cloned();
    if v.is_none() && spell_id != 0 {
        model.spell_tooltip_asks.insert(spell_id);
    }
    v
}

/// The talent lines [`render_spell`] interleaves (`TOOLTIP_TALENT_RANK 0x854a2c`,
/// `TOOLTIP_TALENT_LEARN 0x8549f8`): the white "Rank r/m" after the name, the red requirements
/// while locked, the next-rank block and the green learn hint.
#[derive(Clone, Debug, Default)]
pub(super) struct TalentLines {
    /// `TOOLTIP_TALENT_RANK` ("Rank %d/%d") filled from the player's strings; `None` when they
    /// lack the key, and no rank row shows.
    pub rank_line: Option<String>,
    pub reqs: Vec<String>,
    /// The next rank's spell id, 0 for none, asked from the store while `next_desc` is missing.
    pub next_spell: u32,
    pub next_desc: Option<String>,
    pub learn: bool,
}

/// The talent learn hint's green, `0xff00ff00`.
const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
/// The unmet-requirement red, `0xffff2020` (`0xc0d390`, the item builder's red).
const RED: [f32; 4] = [1.0, 32.0 / 255.0, 32.0 / 255.0, 1.0];

/// The spell builder's options (`0x52e610`'s parameters); `Default` is the plain spell hover.
#[derive(Clone, Copy, Default)]
pub(super) struct SpellRenderOpts {
    /// The aura builder (`0x52f880`): gold name, white description, the dispel class at right.
    pub(super) aura: bool,
    /// `param6` showRank, the gray "Rank N" column: `SetSpell` passes 0, `SetAction` 1.
    pub(super) show_rank: bool,
    /// `param5` altCaster, which drops the totem and reagent lines (`0x52ed43`, `0x52f393`); only
    /// `SetTrainerService` sets it, for a `LEARN_PET_SPELL` service.
    pub(super) alt_caster: bool,
}

fn render_spell(
    lua: &Lua,
    this: &Table,
    v: &SpellTooltipView,
    opts: SpellRenderOpts,
    remaining: Option<String>,
    talent: Option<&TalentLines>,
) -> mlua::Result<()> {
    let SpellRenderOpts {
        aura,
        show_rank,
        alt_caster,
    } = opts;
    // The spell builder writes the name white (`0x530270`), the aura builder gold (`0x530380`).
    let name_color = if aura { GOLD } else { WHITE };
    let right = if aura {
        v.dispel_type.clone().map(|t| (t, GOLD))
    } else {
        v.rank.clone().filter(|_| show_rank).map(|t| (t, GRAY))
    };
    append_line(lua, this, (v.name.clone(), name_color), right, false)?;
    // The talent head: the white rank line, then the red requirements while locked.
    if let Some(t) = talent {
        if let Some(rank) = &t.rank_line {
            append_line(lua, this, (rank.clone(), WHITE), None, false)?;
        }
        for req in &t.reqs {
            append_line(lua, this, (req.clone(), RED), None, true)?;
        }
    }
    if !aura {
        match (&v.cost, &v.range) {
            (Some(c), Some(r)) => append_line(
                lua,
                this,
                (c.clone(), WHITE),
                Some((r.clone(), WHITE)),
                false,
            )?,
            (Some(c), None) => append_line(lua, this, (c.clone(), WHITE), None, false)?,
            (None, Some(r)) => append_line(lua, this, (r.clone(), WHITE), None, false)?,
            (None, None) => {}
        }
        if let Some(ct) = &v.cast_time {
            match &v.cooldown {
                Some(cd) => append_line(
                    lua,
                    this,
                    (ct.clone(), WHITE),
                    Some((cd.clone(), WHITE)),
                    false,
                )?,
                None => append_line(lua, this, (ct.clone(), WHITE), None, false)?,
            }
        }
        if let Some(req) = &v.requires_item {
            let color = if v.item_met { WHITE } else { RED };
            append_line(lua, this, (req.clone(), color), None, false)?;
        }
        if let Some(req) = &v.requires_form {
            let color = if v.form_met { WHITE } else { RED };
            append_line(lua, this, (req.clone(), color), None, false)?;
        }
        // Reagents, white and wrapped, unless altCaster drops them; the totem line it also drops
        // is not built.
        if let Some(reagents) = v.reagents.as_ref().filter(|_| !alt_caster) {
            append_line(lua, this, (reagents.clone(), WHITE), None, true)?;
        }
        // The chance line, white and unwrapped, between the reagents and the description.
        if let Some(chance) = &v.chance {
            append_line(lua, this, (chance.clone(), WHITE), None, false)?;
        }
    }
    let desc = if aura && !v.aura_description.is_empty() {
        &v.aura_description
    } else {
        &v.description
    };
    if !desc.is_empty() {
        let color = if aura { WHITE } else { GOLD };
        append_line(lua, this, (desc.clone(), color), None, true)?;
    }
    // The talent tail: the white `TOOLTIP_TALENT_NEXT_RANK` header (`0x854a10`, pushed at
    // `0x52b2cd`) over the next rank's gold description, then the green `TOOLTIP_TALENT_LEARN`
    // hint (`0x52b362`). Both are the player's own strings; without them the line is skipped.
    if let Some(t) = talent {
        if let Some(next) = &t.next_desc {
            if let Some(header) = crate::strings::global(lua, "TOOLTIP_TALENT_NEXT_RANK") {
                append_line(lua, this, (header, WHITE), None, false)?;
            }
            append_line(lua, this, (next.clone(), GOLD), None, true)?;
        }
        if t.learn {
            if let Some(hint) = crate::strings::global(lua, "TOOLTIP_TALENT_LEARN") {
                append_line(lua, this, (hint, GREEN), None, false)?;
            }
        }
    }
    // The time-remaining line (`SetPlayerBuff` only) is the title's gold `0xffffd200`, not the
    // description's white: a 1.12.1 screenshot reads (255, 210, 0) on it.
    if let Some(rem) = remaining {
        append_line(lua, this, (rem, GOLD), None, false)?;
    }
    Ok(())
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
    {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        clear_content(&mut model, h);
    }
    fire_cleared(lua, h);
    if talent.next_desc.is_none() {
        super::talent::ask_next_rank(lua, talent.next_spell);
    }
    match spell_view_of(lua, spell_id) {
        Some(v) => render_spell(
            lua,
            this,
            &v,
            SpellRenderOpts::default(),
            None,
            Some(&talent),
        )?,
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
        Some((id, name)) => set_spell_by_id(lua, this, id, name, SpellRenderOpts::default(), None),
        None => {
            let h = frame_handle_of(lua, this)?;
            {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                clear_content(&mut model, h);
            }
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
    remaining: Option<String>,
) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        clear_content(&mut model, h);
    }
    fire_cleared(lua, h);
    match spell_view_of(lua, spell_id) {
        Some(v) => render_spell(lua, this, &v, opts, remaining, None)?,
        None => {
            if let Some(name) = fallback_name {
                append_line(lua, this, (name, WHITE), None, false)?;
            }
        }
    }
    super::tooltip::show_or_hide_empty(lua, h);
    Ok(())
}

/// Register the spell and aura content methods on the GameTooltip method table.
pub(super) fn install_methods(lua: &Lua, m: &Table) -> mlua::Result<()> {
    // GameTooltip:SetSpell(bookId, bookType): the spellbook hover. `SetSpell 0x532d10` bounds the
    // 1-based slot to `[0, 0x400)` (`0x532dd4`) and reads the pet book (`0xb6f098`) when `bookType`
    // is "pet" (`0x532e13`), else the player's (`0xb700f0`); the pet flag also picks the cooldown
    // bank it passes to `0x6e2ea0` (`0x532e50`).
    m.set(
        "SetSpell",
        lua.create_function(|lua, (this, book_id, book_type): (Table, u32, Value)| {
            // A non-string `bookType` bails (`0x532dc0`) rather than meaning the player's book.
            let Some(book_type) = book_type.as_string().and_then(|s| s.to_str().ok()) else {
                return Ok(());
            };
            let (spell_id, name) = {
                let model = lua.app_data_mut::<Model>().expect("model app_data");
                match super::spellbook::book_slot(&model, book_id, &book_type) {
                    Some(s) => (s.spell_id, Some(s.name.clone())),
                    None => return Ok(()),
                }
            };
            set_spell_by_id(lua, &this, spell_id, name, SpellRenderOpts::default(), None)
        })?,
    )?;
    // GameTooltip:SetShapeshift(index): the stance-bar hover, the form's spell tooltip.
    m.set(
        "SetShapeshift",
        lua.create_function(|lua, (this, index): (Table, usize)| {
            let (spell_id, name) = {
                let model = lua.app_data_mut::<Model>().expect("model app_data");
                match model.shapeshift_forms.get(index.saturating_sub(1)) {
                    Some(f) => (f.view.spell_id, Some(f.view.name.clone())),
                    None => return Ok(()),
                }
            };
            set_spell_by_id(lua, &this, spell_id, name, SpellRenderOpts::default(), None)
        })?,
    )?;
    // GameTooltip:SetPetAction(index): the pet-bar hover for a spell slot; the stock
    // `PetActionButton_OnEnter` builds a token's tooltip itself (`PetActionBarFrame.lua:290`). A
    // slot with no spell is a no-op.
    m.set(
        "SetPetAction",
        lua.create_function(|lua, (this, index): (Table, usize)| {
            let (spell_id, name) = {
                let model = lua.app_data_mut::<Model>().expect("model app_data");
                match model
                    .pet_bar
                    .slots
                    .get(index.saturating_sub(1))
                    .filter(|s| !s.view.is_token)
                {
                    Some(s) => match s.view.spell_id {
                        Some(id) => (id, s.view.name.clone()),
                        None => return Ok(()),
                    },
                    None => return Ok(()),
                }
            };
            set_spell_by_id(lua, &this, spell_id, name, SpellRenderOpts::default(), None)
        })?,
    )?;
    // GameTooltip:SetPlayerBuff(buffIndex): the aura variant plus the time-remaining line only
    // this entry appends, its text `0x52fa50`'s (`tooltip::duration_text`). The argument is the
    // 0-based cache position `GetPlayerBuff` returns (`BuffFrame.lua:105`), not a filtered
    // ordinal. A miss, negative included, clears and hides the plate as `SetUnitBuff`'s does; a
    // surplus argument is ignored, as in the reference.
    m.set(
        "SetPlayerBuff",
        lua.create_function(|lua, (this, index): (Table, i64)| {
            let now = {
                let g = lua.globals();
                g.get::<f64>("__benilla_now").unwrap_or(0.0)
            };
            let (spell_id, name, remaining_ms) = {
                let model = lua.app_data_mut::<Model>().expect("model app_data");
                let hit = usize::try_from(index)
                    .ok()
                    .and_then(|pos| model.auras.get("player").and_then(|a| a.get(pos)));
                match hit {
                    Some(a) => {
                        // The gate is `untilCancelled`, not a known duration: `0x532b00` skips
                        // the line when the record's `+0xc` is set (`0x532bdf`), a DBC flag that
                        // holds before any `SMSG_UPDATE_AURA_DURATION` lands; a lapsed aura shows
                        // "0 seconds remaining".
                        let ms = (!a.until_cancelled).then(|| {
                            // The reference counts integer milliseconds off `GetTickCount`;
                            // this rounds the float clock to the nearest one.
                            ((a.expiration_time - now) * 1000.0)
                                .round()
                                .clamp(0.0, f64::from(u32::MAX)) as u32
                        });
                        (a.spell_id, a.name.clone(), ms)
                    }
                    // A miss clears: an addon scan that stops when `TextLeft1` reads nil would
                    // otherwise never end.
                    None => (0, None, None),
                }
            };
            // `0x52fa50`'s text from the player's own strings; without them the line is skipped.
            let remaining = remaining_ms.and_then(|ms| {
                let g = lua.globals();
                super::tooltip::duration_text(ms, "SPELL_TIME_REMAINING", true, &|key| {
                    g.get::<String>(key).ok()
                })
            });
            set_spell_by_id(
                lua,
                &this,
                spell_id,
                name,
                SpellRenderOpts {
                    aura: true,
                    ..Default::default()
                },
                remaining,
            )
        })?,
    )?;
    // GameTooltip:SetUnitBuff/SetUnitDebuff(unit, index): the aura variant with no time line; the
    // 1-based index counts within the helpful or harmful list, as `UnitBuff` does.
    for (verb, helpful) in [("SetUnitBuff", true), ("SetUnitDebuff", false)] {
        m.set(
            verb,
            // `index` is a `Value`: the reference's `lua_tonumber` reads nil as 0, which finds no
            // aura, where an `i64` would raise.
            lua.create_function(move |lua, (this, token, index): (Table, String, Value)| {
                let index = match &index {
                    Value::Integer(i) => *i,
                    Value::Number(n) => *n as i64,
                    Value::String(s) => s
                        .to_str()
                        .ok()
                        .and_then(|t| t.parse::<i64>().ok())
                        .unwrap_or(0),
                    _ => 0,
                };
                let hit = {
                    let model = lua.app_data_mut::<Model>().expect("model app_data");
                    let idx = usize::try_from(index.max(1) - 1).unwrap_or(0);
                    model
                        .auras
                        .get(&token)
                        .and_then(|a| a.iter().filter(|a| a.helpful == helpful).nth(idx))
                        .map(|a| (a.spell_id, a.name.clone()))
                };
                // A miss goes through with spell id 0, which clears and hides the plate and
                // records no ask.
                let (spell_id, name) = hit.unwrap_or((0, None));
                set_spell_by_id(
                    lua,
                    &this,
                    spell_id,
                    name,
                    SpellRenderOpts {
                        aura: true,
                        ..Default::default()
                    },
                    None,
                )
            })?,
        )?;
    }
    // GameTooltip:SetTrackingSpell(): the tracking icon's hover, the name gold over the white
    // aura description, as the reference shows "Find Minerals" and as the aura builder's gold
    // wrapper `0x530380` writes it; `0x532c50` itself is untraced. No tracking clears and hides.
    m.set(
        "SetTrackingSpell",
        lua.create_function(|lua, this: Table| {
            let hit = {
                let model = lua.app_data_mut::<Model>().expect("model app_data");
                model
                    .tracking
                    .as_ref()
                    .map(|t| (t.spell_id, t.name.clone()))
            };
            let (spell_id, fallback_name) = hit.unwrap_or((0, None));
            let h = frame_handle_of(lua, &this)?;
            {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                clear_content(&mut model, h);
            }
            fire_cleared(lua, h);
            match spell_view_of(lua, spell_id) {
                Some(v) => {
                    append_line(lua, &this, (v.name.clone(), GOLD), None, false)?;
                    let desc = if !v.aura_description.is_empty() {
                        &v.aura_description
                    } else {
                        &v.description
                    };
                    if !desc.is_empty() {
                        append_line(lua, &this, (desc.clone(), WHITE), None, true)?;
                    }
                }
                None => {
                    // No view yet: the name alone, gold.
                    if let Some(name) = fallback_name {
                        append_line(lua, &this, (name, GOLD), None, false)?;
                    }
                }
            }
            super::tooltip::show_or_hide_empty(lua, h);
            Ok(())
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
                    .selected_quest_detail()
                    .and_then(|d| d.reward_spell.as_ref())
                    .map(|s| (s.spell_id, s.name.clone()))
            };
            set_reward_spell(lua, &this, id)
        })?,
    )?;
    // GameTooltip:SetAction(slot): the action-bar hover by payload kind (`0x5322a0`): a spell
    // (0x00) to the spell builder `0x52e610`, an item (0x80) to the item builder `0x52b650`, a
    // macro (0x40) to `0x52b040`, which fetches it (`0x4f0f40`) and renders only its name, white
    // (`0x5303b0` in colour `0xc0cf60`), or hides with none; 1.12 has no `#showtooltip`.
    m.set(
        "SetAction",
        lua.create_function(|lua, (this, slot): (Table, u32)| {
            let action = {
                let model = lua.app_data_mut::<Model>().expect("model app_data");
                model.actions.get(&slot).cloned()
            };
            let Some(a) = action else { return Ok(()) };
            match a.kind {
                0x00 => set_spell_by_id(
                    lua,
                    &this,
                    a.action,
                    None,
                    SpellRenderOpts {
                        show_rank: true,
                        ..Default::default()
                    },
                    None,
                ),
                0x80 => {
                    let f: mlua::Function = this.get("BenillaSetItemById")?;
                    f.call::<()>((this.clone(), a.action))
                }
                0x40 => {
                    let h = frame_handle_of(lua, &this)?;
                    let name = {
                        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                        clear_content(&mut model, h);
                        // The same 1..36 lookup as `GetActionText` and the bar icon, so the
                        // three agree on a slot's macro.
                        model.macros.get(a.action as usize).map(|m| m.name.clone())
                    };
                    fire_cleared(lua, h);
                    if let Some(name) = name {
                        append_line(lua, &this, (name, WHITE), None, false)?;
                    }
                    super::tooltip::show_or_hide_empty(lua, h);
                    Ok(())
                }
                _ => Ok(()),
            }
        })?,
    )?;

    // GameTooltip:SetTrainerService(index): the trainer detail icon's hover
    // (`Blizzard_TrainerUI.xml:452`); list rows have no tooltip. `0x5338b0` writes no line itself
    // and hands a shared builder the subject the app resolved (`TrainerService::tooltip`). `index`
    // is a visible row, headers included; a header is a no-op.
    m.set(
        "SetTrainerService",
        lua.create_function(|lua, (this, index): (Table, usize)| {
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
                    let f: mlua::Function = this.get("BenillaSetItemById")?;
                    f.call::<()>((this.clone(), item_id))
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
                    None,
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
        lua.create_function(|lua, (this, craft_index): (Table, usize)| {
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
                    let f: mlua::Function = this.get("BenillaSetItemById")?;
                    f.call::<()>((this.clone(), item_id))
                }
                CraftTooltip::Spell(spell_id) => {
                    set_spell_by_id(lua, &this, spell_id, None, SpellRenderOpts::default(), None)
                }
            }
        })?,
    )?;
    Ok(())
}
