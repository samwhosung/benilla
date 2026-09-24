//! The engine **spell/aura tooltip channel** (decision 0274 P2) — the verified line law of the
//! spell builder `0x52e610` and the aura builder `0x52f880` (the 0276 fold-back):
//!
//! - name | rank (gray) — one double line. The name's colour is the BUILDER's: **white** from
//!   the spell builder (`0x530270`), **gold** from the aura builder (`0x530380`, the gold
//!   wrapper) — the same split `SetTrackingSpell` shows;
//! - **Cost | Range** — ONE double line (either side may be absent);
//! - **CastTime | Cooldown** — ONE double line; a passive spell simply omits it (there is NO
//!   "Passive" text in the 1.12 builder);
//! - required tool / form — the equipped-item-class line then the stance line, each white when
//!   met and red when not;
//! - reagents — inline red per missing item (the one builder that uses the `|cffff2020` escape;
//!   joins when a reagent feed exists);
//! - description — gold, wrapped. An AURA's description is **white** (byte-verified difference),
//!   and only `SetPlayerBuff` appends the duration-remaining line, which is **gold** again
//!   (`0xffffd200`, the title's gold — B62; see [`render_spell`]'s tail).
//!
//! The engine renders VIEWS ([`SpellTooltipView`]) the app resolves at push time — the $-token
//! substitution (values off Spell.dbc + the player's level), cast-time/duration/range text —
//! because the catalogs and the token engine live app-side; the engine holds no DBC knowledge.
//! Views are keyed by spell id in an ask-once store (the item-template store's pattern): a
//! renderer miss records the id, the app resolves and pushes, the hover's re-enter repaints.

use mlua::{Lua, Table, Value};

use super::object::frame_handle_of;
use super::tooltip::{append_line, clear_content, fire_cleared};
use super::{CraftTooltip, Model, TrainerTooltip};

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
/// The rank column's gray — byte-verified `0xff808080` (0276).
const GRAY: [f32; 4] = [128.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0, 1.0];
/// The description gold — byte-verified `0xffffd200` (0276).
const GOLD: [f32; 4] = [1.0, 210.0 / 255.0, 0.0, 1.0];

/// One spell's tooltip view — every string app-resolved (the $-engine's output for the
/// description; the cost/range/cast/cooldown texts off the DBC catalogs).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpellTooltipView {
    pub name: String,
    /// "Rank N" — the gray right column of the name line, on the SPELL variant only.
    pub rank: Option<String>,
    /// "Magic" / "Curse" / "Disease" / "Poison" — the AURA variant's right column (`0x52f8e5`),
    /// the `SpellDispelType.dbc` name of the spell's dispel class gated by that table's `[+0x28]`
    /// flag, so an undispellable aura (Stealth) carries `None`. Rendered GOLD, like the aura name
    /// it shares its line with — not the spell variant's gray.
    pub dispel_type: Option<String>,
    /// "35 Mana" / "20 Rage" / "20 Health" / "11 Health, plus 5 per sec" — the cost cell: the
    /// RESOLVED cost through the power-type key array with the health fallback (1074).
    pub cost: Option<String>,
    /// "30 yd range" — the range cell.
    pub range: Option<String>,
    /// "1.5 sec cast" / "Instant cast" / "Instant" / "Next melee" / "Attack speed" /
    /// "Channeled" — `None` = a passive spell: the whole casttime|cooldown line is omitted
    /// (the verified law; never a "Passive" text line).
    pub cast_time: Option<String>,
    /// "15 sec cooldown" — the cooldown cell: `max(RecoveryTime, CategoryRecoveryTime)` (the 0276
    /// line law, `SPELL_RECAST_TIME_SEC 0x854ed4` — Charge's 15 s lives in the CATEGORY column).
    pub cooldown: Option<String>,
    /// "Requires Wands" — the equipped-item-class half (`SPELL_EQUIPPED_ITEM 0x854e94`), over
    /// `EquippedItemClass`/`EquippedItemSubClassMask`: white when [`Self::item_met`], red when
    /// not. Sits ABOVE [`Self::requires_form`] (the law's tool-then-form order).
    pub requires_item: Option<String>,
    /// Whether some WORN item satisfies the class + subclass mask (the app re-pushes views when
    /// the equipped set changes, so the color tracks live swaps).
    pub item_met: bool,
    /// "Requires Battle Stance" — the required-form line (`SPELL_REQUIRED_FORM 0x854e64` over
    /// the `Stances` mask): white when [`Self::form_met`], red when not.
    pub requires_form: Option<String>,
    /// Whether the player's CURRENT shapeshift form satisfies the mask (the app re-pushes views
    /// on a form change, so the color tracks live stance switches).
    pub form_met: bool,
    /// "Reagents: Light Feather" — `SPELL_REAGENTS 0x854e54` ("Reagents: ", no format slot) +
    /// the 8 reagent slots. Rendered WHITE and wrapped; a reagent the player is short of carries
    /// the builder's own **inline** `|cffff2020` escape (the spell builder is the one builder
    /// that colors mid-line), so the composed string arrives paint-ready from the app.
    pub reagents: Option<String>,
    /// "2.62% chance to dodge" — `CHANCE_TO_DODGE 0x854e44` and its siblings, selected by the
    /// spell's `Effect[0]` and rendered white and unwrapped, BELOW the reagents and ABOVE the
    /// description. The
    /// percentage is the player's own live avoidance/crit field, so the app re-pushes as it moves.
    pub chance: Option<String>,
    /// The $-substituted description — gold + wrapped for spells.
    pub description: String,
    /// The $-substituted AURA description (`Spell.dbc AuraDescription`) — the buff hover's
    /// white text (byte-verified: the aura builder reads the aura column). Falls back to
    /// `description` when empty.
    pub aura_description: String,
}

impl super::UiScript {
    /// Store (or replace) a spell's tooltip view — the app's push half of the ask-once flow.
    pub fn set_spell_tooltip(&mut self, spell_id: u32, view: SpellTooltipView) {
        let mut model = self.model_mut();
        model.spell_tooltip_asks.remove(&spell_id);
        model.spell_tooltips.insert(spell_id, view);
    }

    /// Drain the spell ids the renderers asked for that the store didn't have.
    pub fn take_spell_tooltip_asks(&mut self) -> Vec<u32> {
        self.model_mut().spell_tooltip_asks.drain().collect()
    }
}

/// Look up the store; a miss records the ask. `pub(super)` for the talent tooltip's shared use
/// (its display + next-rank spells ride this same ask-once channel — decision 0304).
pub(super) fn spell_view_of(lua: &Lua, spell_id: u32) -> Option<SpellTooltipView> {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let v = model.spell_tooltips.get(&spell_id).cloned();
    if v.is_none() && spell_id != 0 {
        model.spell_tooltip_asks.insert(spell_id);
    }
    v
}

/// The talent interleave for [`render_spell`] (decision 0304; the builder's own talent params —
/// `TOOLTIP_TALENT_RANK 0x854a2c` / `TOOLTIP_TALENT_LEARN 0x8549f8`): the white "Rank r/m" after
/// the name, the red requirement lines while locked (position CONFIRMED — decision 0305's residue:
/// matches the builder law, after the rank line), the "Next rank:" block, and the green learn hint.
#[derive(Clone, Debug, Default)]
pub(super) struct TalentLines {
    /// `TOOLTIP_TALENT_RANK` = "Rank %d/%d", already filled by the caller off the player's own
    /// string table. `None` = the table does not carry the key, and the plate shows no rank row
    /// (decision 2045 — never an invented one).
    pub rank_line: Option<String>,
    pub reqs: Vec<String>,
    /// The next rank's spell id (0 = none) — asked from the spell store when its description
    /// hasn't landed yet, so the hover's re-enter completes the block.
    pub next_spell: u32,
    pub next_desc: Option<String>,
    pub learn: bool,
}

/// `TOOLTIP_TALENT_LEARN 0x8549f8`'s green — the shared talent-learn green of the tooltip color
/// table (`0xff00ff00`).
const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
/// The unmet-requirement red — `0xc0d390 = ffff2020` (the item builder's own RED value).
const RED: [f32; 4] = [1.0, 32.0 / 255.0, 32.0 / 255.0, 1.0];

/// Render one spell view — the verified law (module doc). `aura` renders the aura variant:
/// white description, plus the caller-supplied duration-remaining line (`SetPlayerBuff` only).
/// `talent` interleaves the talent lines ([`TalentLines`] doc).
/// The builder's parameter vector, named as the byte law names it (`0x52e610`'s param3..param8).
/// These were three positional `bool`s at a
/// 7-argument call site, which is exactly the shape that gets silently transposed; `Default` is the
/// plain spell hover every caller but two wants.
#[derive(Clone, Copy, Default)]
pub(super) struct SpellRenderOpts {
    /// Render through the AURA builder (`0x52f880`) rather than the spell builder: gold name, white
    /// description, the dispel-class right column.
    pub(super) aura: bool,
    /// `param6` showRank — the gray "Rank N" right column. `SetSpell` passes 0 (the spellbook hover
    /// never shows it), `SetAction` passes 1.
    pub(super) show_rank: bool,
    /// `param5` altCaster — **one** gate suppressing **both** the totems and the reagents lines
    /// (byte-verified at the two branch sites `0x52ed43` and `0x52f393`). Set only by
    /// `SetTrainerService`, and only when the matched learn-wrapper slot was `LEARN_PET_SPELL`: a
    /// pet-training service shows neither block.
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
    // The name colour splits by BUILDER, byte-verified: the spell builder `0x52e610` writes its
    // name line through `0x530270` (white), the aura builder `0x52f880` through `0x530380` — the
    // GOLD wrapper. SetTrackingSpell's gold,
    // already pinned by the director's own A/B, is the same wrapper.
    let name_color = if aura { GOLD } else { WHITE };
    // The name line's RIGHT column splits by builder too. The spell builder's is the gray "Rank N",
    // and it shows only when the CALLER asks (byte-verified: SetSpell passes param6=0 — the
    // spellbook hover never shows "Rank N"; SetAction passes 1). The aura builder's is the dispel
    // class ("Magic" on Ice Armor — `0x52f8e5`), and it is GOLD, not gray: a buff never shows a
    // rank.
    let right = if aura {
        v.dispel_type.clone().map(|t| (t, GOLD))
    } else {
        v.rank.clone().filter(|_| show_rank).map(|t| (t, GRAY))
    };
    append_line(lua, this, (v.name.clone(), name_color), right, false)?;
    // The talent head: "Rank r/m" (builder line 2, TOOLTIP_TALENT_RANK white) + the red
    // requirement lines while locked (position CONFIRMED, decision 0305 — TalentLines doc).
    if let Some(t) = talent {
        if let Some(rank) = &t.rank_line {
            append_line(lua, this, (rank.clone(), WHITE), None, false)?;
        }
        for req in &t.reqs {
            append_line(lua, this, (req.clone(), RED), None, true)?;
        }
    }
    if !aura {
        // Cost | Range — one line, either side optional.
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
        // CastTime | Cooldown — one line; a passive spell (cast_time None) omits it whole.
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
        // Required tool / form (`SPELL_EQUIPPED_ITEM 0x854e94` / `SPELL_REQUIRED_FORM 0x854e64`):
        // the equipped-item-class line first, then the stance line. Each white when met, red when
        // not.
        if let Some(req) = &v.requires_item {
            let color = if v.item_met { WHITE } else { RED };
            append_line(lua, this, (req.clone(), color), None, false)?;
        }
        if let Some(req) = &v.requires_form {
            let color = if v.form_met { WHITE } else { RED };
            append_line(lua, this, (req.clone(), color), None, false)?;
        }
        // Reagents (`SPELL_REAGENTS 0x854e54`): white + wrapped, the missing entries inline-red
        // inside the text.
        // Suppressed wholesale by altCaster — the same gate that hides the totems block, which we
        // have no feed for yet, so this is the only half of it that is observable here.
        if let Some(reagents) = v.reagents.as_ref().filter(|_| !alt_caster) {
            append_line(lua, this, (reagents.clone(), WHITE), None, true)?;
        }
        // Chance to dodge/parry/block/crit (`CHANCE_TO_DODGE 0x854e44` and its siblings) — white,
        // NOT wrapped, and it sits here: below the reagents, above the description.
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
    // The talent tail: the TOOLTIP_TALENT_NEXT_RANK header (white, `0x854a10` pushed at
    // `0x52b2cd`) over the next rank's gold description, and the green learn hint (builder
    // line 13, TOOLTIP_TALENT_LEARN `0x8549f8` at `0x52b362`). Both are keys into the player's
    // own string table (decision 2045); an install without them shows the description alone
    // rather than a sentence of ours.
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
    // The duration-remaining line (`SetPlayerBuff` only) is GOLD `0xffffd200` — the same gold as
    // the aura title it sits under, NOT the description's white. Measured off the reporter's own
    // 1.12.1 reference shot for B62 (`media/1530672450247856148`, Ice Armor in Wetlands): the
    // "29 minutes remaining" glyphs read exactly `(255, 210, 0)`, pixel-identical to that shot's
    // "Ice Armor" / "Magic" title row, while its description rows read `(255, 255, 255)`. Ours
    // rendered it white (`media/1530672304877469696`, `(255, 255, 255)`) — that white *is* B62.
    if let Some(rem) = remaining {
        append_line(lua, this, (rem, GOLD), None, false)?;
    }
    Ok(())
}

/// The talent tooltip's entry (decision 0304 — `GameTooltip:SetTalent`'s render half): the
/// display spell through the shared store + the talent interleave. A missing next-rank view is
/// re-asked so the hover's re-enter completes the block.
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
            // The view hasn't landed: show the talent head alone (the ask is recorded; the
            // hover's re-enter repaints complete) — the spell channel's own fallback shape.
            if let Some(rank) = &talent.rank_line {
                append_line(lua, this, (rank.clone(), WHITE), None, false)?;
            }
        }
    }
    super::tooltip::show_or_hide_empty(lua, h);
    Ok(())
}

/// Shared entry: clear, render (or record the ask and show nothing but the name if the caller
/// knows one), show.
/// The reward-spell hover: the spell by id with the quest's own name as the fallback, or an
/// empty tooltip.
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

/// Register the spell/aura content channels into the GameTooltip kind method table.
pub(super) fn install_methods(lua: &Lua, m: &Table) -> mlua::Result<()> {
    // GameTooltip:SetSpell(bookId, bookType) — the spellbook hover: the 1-based book slot resolves
    // through the named book's state to a spell id.
    //
    // **`bookType` decides which book**, exactly like every `bookType`-taking global
    // (`super::spellbook::book_slot`). Byte-verified in `SetSpell 0x532d10`, which is its own
    // implementation of the same fork rather than a caller of the shared parser: arg2 → number,
    // `- 1`, bounded `[0, 0x400)` (`0x532dd4`-`0x532df4`); arg3 → string, compared against the
    // literal `"pet"` at `0x846960` (`0x532e13`); match takes `[4*i + 0xb6f098]` — the PET book —
    // and sets `isPet = 1` (`0x532e1c`), everything else takes `[4*i + 0xb700f0]` (`0x532e2a`).
    // That `isPet` then rides into `0x6e2ea0` as the cooldown BANK (`0x532e50`), the same bank
    // split 1031 built.
    //
    // Before the fork, a pet-book hover indexed the PLAYER's slot list, so hovering the imp's first
    // spell showed the player's first spell — "Attack", crit line and all (decision 1050).
    m.set(
        "SetSpell",
        lua.create_function(|lua, (this, book_id, book_type): (Table, u32, Value)| {
            // The reference requires a STRING third argument and bails otherwise (`0x532dc0`'s
            // `lua_isstring(3)` → `je` out); a non-string is therefore not "the player's book".
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
    // GameTooltip:SetShapeshift(index) — the stance-bar hover (the form's own spell tooltip).
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
    // GameTooltip:SetPetAction(index) — the pet-bar hover (decision 0982). Only ever reached for a
    // SPELL slot: the reference's `PetActionButton_OnEnter` builds a token slot's tooltip inline
    // from `tooltipName`/`tooltipSubtext` and never calls this. A slot with no spell (a token, an
    // empty slot, an out-of-range index) is a no-op, leaving whatever was shown — the same shape
    // as SetSpell's out-of-range.
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
    // GameTooltip:SetPlayerBuff(buffIndex) — the buff-bar hover: the aura variant (white
    // AuraDescription) + the duration-remaining line only this entry point appends (byte-verified;
    // remaining computed live off the aura's GetTime expiry). The line's TEXT is no longer interim:
    // `0x52fa50`'s four-arm ladder and its rounding pin it, and
    // `tooltip::duration_text` is it.
    //
    // **The argument is a 1.12 CACHE POSITION, not a filtered ordinal** — the same 0-based handle
    // `GetPlayerBuff` returns and every `GetPlayerBuff*` sibling consumes (see
    // `super::aura`'s header). `ref-BuffFrame.lua:105` is the pin: `GameTooltip:SetPlayerBuff(buffIndex)`
    // where `buffIndex` came straight out of `GetPlayerBuff`, never from the button's own id.
    //
    // This corrects a real defect: the binding previously read a 1-based index within the
    // sign-filtered list, so of the corpus's 21 call sites — all of which pass a cache position —
    // every position >= 1 resolved one aura too early, no debuff was ever reachable (the sign
    // defaulted to helpful), and `SetPlayerBuff(-1)` showed the FIRST buff instead of nothing.
    // That last one is load-bearing: `BigWigs/Raids/Naxxramas/Loatheb.lua:260-271` feeds an
    // unchecked `GetPlayerBuff(i, "HARMFUL")` straight in and terminates its scan on the tooltip's
    // first line going nil.
    //
    // A miss (out of range, negative, empty cache) routes through the shared entry with spell id 0,
    // exactly like SetUnitBuff's: content clears and the plate hides, never a stale tooltip left
    // showing. Any surplus argument is ignored — `CT_BuffMod/CT_BuffFrame.lua:151` passes a filter
    // string the reference's own binding never reads.
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
                        // The gate is `untilCancelled`, NOT "does this aura have a duration yet".
                        // `0x532b00` skips the whole duration block when the
                        // cache record's `+0xc` is set (`532bda: 8b 46 0c` / `532bdf: 75 2d`), so
                        // a permanent aura shows title + description and nothing more. That flag
                        // is DBC-derived, so it is already right on the frame an aura appears —
                        // before any `SMSG_UPDATE_AURA_DURATION` lands — which is exactly why
                        // `AuraState::until_cancelled`'s own doc calls it a different question
                        // from `expiration_time == 0.0`. Gating on the duration (what this did)
                        // blanked the line for a timed aura's first frames, and hid the
                        // reference's own "0 seconds remaining" once one lapsed.
                        let ms = (!a.until_cancelled).then(|| {
                            // The reference counts integer milliseconds off GetTickCount; ours is
                            // a float second clock, so this is the nearest millisecond to it.
                            ((a.expiration_time - now) * 1000.0)
                                .round()
                                .clamp(0.0, f64::from(u32::MAX)) as u32
                        });
                        (a.spell_id, a.name.clone(), ms)
                    }
                    // The miss clears rather than leaving the previous plate up — the same shape
                    // SetUnitBuff uses, and the one Loatheb's scan depends on: it breaks its loop
                    // when TextLeft1 reads nil, so a stale line would never let it terminate.
                    None => (0, None, None),
                }
            };
            // The text is `0x52fa50`'s, over the player's own GlobalStrings — never a string of
            // ours. Off an install the table is absent and the line simply does not render.
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
    // GameTooltip:SetUnitBuff(unit, index) / SetUnitDebuff(unit, index) — the target frame's aura
    // hover: the same aura variant (white AuraDescription), WITHOUT the duration-remaining line —
    // byte-verified, only SetPlayerBuff appends it (and no other unit has a duration on the 1.12
    // wire anyway). The index counts within the sign-filtered list, the UnitBuff/UnitDebuff
    // convention.
    for (verb, helpful) in [("SetUnitBuff", true), ("SetUnitDebuff", false)] {
        m.set(
            verb,
            // `index` is `Value`, not `i64`, and that is a fidelity fix rather than laxity — the
            // same correction `SetTexture` already carries. A C binding reads what it wants off
            // the Lua stack: `lua_tonumber` on nil yields 0, which finds no aura and shows
            // nothing. Typing it `i64` made us RAISE on a call the real client accepts silently.
            //
            // Found by the use-probe: `CT_AssistFrameDebuff1:OnEnter` calls
            // `SetUnitDebuff(unit, this:GetID())` and the id is nil on a frame CT_UnitFrames
            // created without one. It only fires on hover, so nothing before the probe saw it.
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
                // A miss (index past the list, unknown token) still routes through the shared
                // entry with spell id 0: content clears and the empty plate hides — never a
                // stale previous tooltip left showing. Id 0 records no ask.
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
    // GameTooltip:SetTrackingSpell() — the minimap tracking icon's hover: NAME gold over a white
    // (aura-)description, pinned by the director's reference A/B (2026-07-20: "Find Minerals"
    // gold over white "Finding Minerals."). That is just the AURA builder's shape — its name line
    // rides the gold wrapper `0x530380` too — so this is no longer a one-off; `0x532c50`'s body
    // is still unconfirmed in the binary, so read it before extending BEYOND that shape. No
    // duration-remaining line (only SetPlayerBuff appends one), and no tracking active clears +
    // hides, like the SetUnitBuff miss path.
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
                    // The view hasn't landed (ask recorded; the re-enter repaints): the name
                    // alone, in the same gold.
                    if let Some(name) = fallback_name {
                        append_line(lua, &this, (name, GOLD), None, false)?;
                    }
                }
            }
            super::tooltip::show_or_hide_empty(lua, h);
            Ok(())
        })?,
    )?;
    // GameTooltip:SetAction(slot) — the action-bar hover: pure delegation by payload kind
    // (`0x5322a0`, byte-verified: SPELL 0x00 → the spell builder `0x52e610`, ITEM 0x80 → the item
    // builder `0x52b650`, MACRO 0x40 → `0x52b040`).
    //
    // The MACRO arm is the reference's whole `0x52b040`: fetch the record (`0x4f0f40`), and either
    // hide (no record) or render ONE line — the macro's NAME (`rec+0x24`) — through the
    // single-coloured-line wrapper `0x5303b0` in the "normal" colour `0xc0cf60`, which is WHITE
    // (`0xffffffff`, never rewritten). Not the spell title's gold, and nothing about the bound
    // spell: 1.12 has no
    // `#showtooltip`. This arm was a `_ => Ok(())` left from before 0983 shipped macros, which is
    // why a macro on the bar hovered to nothing (the director, 2026-08-27, after 1636).
    // GameTooltip:SetQuestRewardSpell() / SetQuestLogRewardSpell() — the hover of the reward
    // slot stock marks `rewardType = "spell"` (QuestFrameTemplates.xml:150, QuestLogFrame.xml:115;
    // bindings 0x535bb0 / 0x535c60, self only, 0 returns): the quest's reward spell by id through
    // the spell renderer, an empty tooltip when the quest rewards none.
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
                    // Route through the shared item renderer (the id-keyed entry).
                    let f: mlua::Function = this.get("BenillaSetItemById")?;
                    f.call::<()>((this.clone(), a.action))
                }
                0x40 => {
                    let h = frame_handle_of(lua, &this)?;
                    let name = {
                        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                        clear_content(&mut model, h);
                        // The same 1..36 index lookup `GetActionText` and the bar's icon arm
                        // make, so the three can never disagree about which macro a slot holds.
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

    // GameTooltip:SetTrainerService(index) — the trainer detail-icon hover (ref
    // `Blizzard_TrainerUI.xml:452`, whose OnEnter is SetOwner(this,"ANCHOR_RIGHT") +
    // SetTrainerService(ClassTrainerFrame.selectedService) + Show(); the LIST ROWS carry no tooltip
    // at all).
    //
    // The binding is a **selector, not a renderer**: `0x5338b0` emits no line of its own (verified
    // negative — none of the four AddLine helpers appears in its extent) and hands one of the two
    // shared builders a subject. The subject is decided app-side, because the law reads `Spell.dbc`
    // fields the engine cannot see, and arrives pre-resolved as `TrainerService::tooltip`. All this
    // does is pick the renderer — which is exactly what the reference binding does.
    //
    // `index` is a **VISIBLE** row index (headers interleave), resolved through the same mapping
    // every other trainer getter uses; a header row is a no-op.
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
                // Route through the shared item renderer, the way the reference routes into
                // `0x52b650`. No fallback name: an item id of 0 or a template still in flight
                // renders an EMPTY tooltip, which is the builder's own early-out.
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
    // GameTooltip:SetCraftSpell(craftIndex) — the Craft window's detail-icon hover (ref
    // `CraftIcon`'s OnEnter, `Blizzard_CraftUI.xml:566`). `SetTrainerService`'s structural twin and
    // its law's opposite (`SetCraftSpell 0x533e90`): a selector into
    // the same two shared builders, deciding on the RECIPE's own effect columns rather than a
    // taught spell's attributes. The subject arrives pre-resolved as `CraftRecipe::tooltip`.
    //
    // This replaced a v1 two-line "name white, description gold" render. That was wrong in both
    // halves: `0x533e90` funnels into `0x52e610`/`0x52b650` like every other content binding, so
    // two lines were eight-plus short, and on a `LEARN_SPELL` or `CREATE_ITEM` recipe it was
    // describing the wrong subject entirely. `craft_index` is a raw recipe position — the Craft
    // window is FLAT, with no headers to interleave (unlike the tradeskill list).
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
