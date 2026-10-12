//! The tradeskill bindings behind the stock `Blizzard_TradeSkillUI`: the app pushes a snapshot
//! ([`UiScript::set_trade_skill`]) with recipes resolved from `Spell.dbc`, `SkillLineAbility.dbc`
//! and bag counts, and drains the intents `DoTradeSkill` and `CloseTradeSkill` queue. The engine
//! owns the grouped, filtered list (`0x4fca20`); every Lua index, `SetTradeSkillItem`'s included,
//! is 1-based into its visible rows ([`recipe_at`]). The filter, fold, repeat and close verbs fire
//! their events inside the call, as the reference's do.

use std::collections::HashSet;

mod api;
mod view;

#[cfg(test)]
mod tests;

use view::build_groups;

pub(super) use api::install;
pub(crate) use view::recipe_at;

/// A recipe's difficulty band, computed app-side: gray at or above trivialHigh, green from the
/// low/high midpoint, yellow from trivialLow, orange below (`0x4fca20`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TradeSkillDifficulty {
    /// Orange: a near-certain skill-up.
    Optimal,
    /// Yellow: a good skill-up chance.
    Medium,
    /// Green: a fading skill-up chance.
    Easy,
    /// Gray: no skill-up.
    Trivial,
}

impl TradeSkillDifficulty {
    /// A recipe row's `type` in `GetTradeSkillInfo`, a `TradeSkillTypeColor` key
    /// (`Blizzard_TradeSkillUI.lua:7-10`).
    pub fn as_str(self) -> &'static str {
        match self {
            TradeSkillDifficulty::Optimal => "optimal",
            TradeSkillDifficulty::Medium => "medium",
            TradeSkillDifficulty::Easy => "easy",
            TradeSkillDifficulty::Trivial => "trivial",
        }
    }

    /// The tier byte recipes sort by, ascending (`0x4fd380`). The Craft window's comparators key on
    /// the same order (`row[+0xc]`, shifted by one for a "none" tier), so it shares this.
    pub(crate) fn tier(self) -> u8 {
        match self {
            TradeSkillDifficulty::Optimal => 0,
            TradeSkillDifficulty::Medium => 1,
            TradeSkillDifficulty::Easy => 2,
            TradeSkillDifficulty::Trivial => 3,
        }
    }
}

/// One reagent of a recipe, as `GetTradeSkillReagentInfo` reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct TradeSkillReagent {
    /// The item entry `SetTradeSkillItem(i, j)` resolves through.
    pub item: u32,
    /// `None` until the item template arrives.
    pub name: Option<String>,
    /// `None` until the item template arrives.
    pub icon: Option<String>,
    /// How many this recipe consumes.
    pub need: u32,
    /// How many the player's bags hold.
    pub have: u32,
}

/// One recipe: a known spell with `SPELL_ATTR_IS_TRADESKILL`, resolved app-side from `Spell.dbc`,
/// `SkillLineAbility.dbc` and bag counts.
#[derive(Clone, Debug, PartialEq)]
pub struct TradeSkillRecipe {
    /// The product's `(ItemClass, ItemSubClass)` and `ItemSubClass.dbc` name, the header key
    /// (`0x55ba30`); `None` until the product's template arrives.
    pub group: Option<(u32, u32, String)>,
    /// What `DoTradeSkill` casts, one `CMSG_CAST_SPELL` per item: the packet has no count.
    pub spell_id: u32,
    pub name: String,
    pub difficulty: TradeSkillDifficulty,
    /// The least `have / need` over the reagents, reported as `numAvailable`.
    pub num_available: u32,
    /// The product's icon, else the spell's; `None` while neither has arrived.
    pub icon: Option<String>,
    pub min_made: u32,
    pub max_made: u32,
    /// Remaining cooldown in seconds; `None` when ready.
    pub cooldown_secs: Option<u64>,
    /// The created item, 0 for a pure-effect recipe.
    pub product_item: u32,
    /// The product's `InventoryType`, which the inv-slot filter folds to slot bits through
    /// `DAT_00809200` (`0x4fcee6`); 0 takes the catch-all bit.
    pub product_inv_type: u32,
    /// The product's `ItemLevel`, the sort key between tier and name (`record+0x14`, `0x7c9640`).
    pub product_item_level: u32,
    /// In display order.
    pub reagents: Vec<TradeSkillReagent>,
    /// The Requirements line's `(tool name, have)` pairs.
    pub tools: Vec<(String, bool)>,
}

/// One open tradeskill window, pushed whole by the app.
#[derive(Clone, Debug, PartialEq)]
pub struct TradeSkillState {
    /// The `SkillLine.dbc` id, the persistence key (`0xbde064`).
    pub line: u32,
    pub line_name: String,
    pub rank: u32,
    pub max_rank: u32,
    /// Unordered: the engine sorts them (`0x4fca20`), and Lua indexes the visible rows, not this.
    pub recipes: Vec<TradeSkillRecipe>,
}

impl super::UiScript {
    /// Pushes the open window's snapshot, or `None` on close; the app re-pushes on any change.
    ///
    /// Persistence follows the reference (`0x4fc910`, keyed by `0xbde064`): a different skill line
    /// resets filters, folds and selection, and the same line keeps them across a close. The
    /// selection rides its spell id (`0xbde044`); fold and subclass keys are pruned to live groups,
    /// while the inv-slot mask (`0x84dd64`) is never pruned.
    /// `None` marks the window closed and keeps the list and selection, as the close does
    /// (`0x4fc870`).
    pub fn set_trade_skill(&mut self, state: Option<TradeSkillState>) {
        let mut model = self.model_mut();
        model.trade_skill_open = state.is_some();
        match state {
            None => {}
            Some(s) => {
                if model.trade_skill_last_line != s.line {
                    model.trade_skill_collapsed.clear();
                    model.trade_skill_subclass_hidden.clear();
                    model.trade_skill_invslot_mask = u32::MAX;
                    model.trade_skill_selected_spell = 0;
                    model.trade_skill_last_line = s.line;
                }
                model.trade_skill_selection = (model.trade_skill_selected_spell != 0)
                    .then(|| {
                        s.recipes
                            .iter()
                            .position(|r| r.spell_id == model.trade_skill_selected_spell)
                    })
                    .flatten()
                    .map_or(0, |i| (i + 1) as u32);
                let live: HashSet<(u32, u32)> = build_groups(&s.recipes)
                    .into_iter()
                    .map(|g| g.key)
                    .collect();
                model.trade_skill_collapsed.retain(|k| live.contains(k));
                model
                    .trade_skill_subclass_hidden
                    .retain(|k| live.contains(k));
                model.trade_skill = Some(s);
            }
        }
    }

    /// Drains the `(spell id, count)` crafts `DoTradeSkill` queued; the app's repeat loop
    /// (`0x4fd7b0`) sends one cast per item.
    pub fn take_trade_skill_dos(&mut self) -> Vec<(u32, u32)> {
        std::mem::take(&mut self.model_mut().trade_skill_dos)
    }

    /// Whether `CloseTradeSkill` ran since the last drain; the close sends no packet.
    pub fn take_trade_skill_close(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().trade_skill_close)
    }

    /// Whether the VM holds the window open: `CloseTradeSkill` closes it in the verb, so a feed
    /// that finds it closed owes no `TRADE_SKILL_CLOSE` of its own.
    pub fn trade_skill_open(&self) -> bool {
        self.model_ref().trade_skill_open
    }

    /// The post-cast hook `0x4fd7b0`, run for each of our own `SMSG_SPELL_GO`s without flag
    /// `0x41` (`0x6e81de`): the armed spell counts down (`0x4fd7ca`) and, while any are left,
    /// recasts and signals `UPDATE_TRADESKILL_RECAST` (`0x4fd7dc`); the last one ends silently
    /// with the spell still armed (`0x4fd7cf`). Any other cast, or nothing left, disarms
    /// (`0x4fd7e6` → `0x4fd790`). Returns the spell to recast, which the app sends.
    pub fn trade_skill_spell_go(&mut self, spell_id: u32) -> Option<u32> {
        super::tick::verb(&self.lua, |model| {
            let armed = model.trade_skill_repeat_spell;
            if armed == 0 || model.trade_skill_repeat_count == 0 || armed != spell_id {
                disarm_repeat(model);
                return None;
            }
            model.trade_skill_repeat_count -= 1;
            if model.trade_skill_repeat_count == 0 {
                return None;
            }
            model.raise("UPDATE_TRADESKILL_RECAST", Vec::new());
            Some(armed)
        })
    }

    /// A failed or interrupted cast of the armed spell disarms the repeat with its own signal
    /// (`0x6e1a3c`-`0x6e1a55` → `0x4fd790`), so a later close finds nothing to disarm.
    pub fn trade_skill_cast_failed(&mut self, spell_id: u32) {
        super::tick::verb(&self.lua, |model| {
            if spell_id == model.trade_skill_repeat_spell {
                disarm_repeat(model);
            }
        });
    }

    /// The armed repeat as `(spell, count)`, the reference's `0xbde05c` and `0xbde060`.
    pub fn trade_skill_repeat(&self) -> (u32, u32) {
        let model = self.model_ref();
        (
            model.trade_skill_repeat_spell,
            model.trade_skill_repeat_count,
        )
    }
}

/// `0x4fd790`: an armed repeat disarms and signals `UPDATE_TRADESKILL_RECAST`; the count stays.
pub(crate) fn disarm_repeat(model: &mut super::Model) {
    if model.trade_skill_repeat_spell != 0 {
        model.trade_skill_repeat_spell = 0;
        model.raise("UPDATE_TRADESKILL_RECAST", Vec::new());
    }
}
