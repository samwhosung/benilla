//! The item use dispatcher's gates (`0x5d8d00`, `CGItem::Use`) that sit ahead of its arms that
//! send or lock, judged in the call that reaches them.

use super::container::UseFork;
use super::Model;

/// The line `CGItem::Use` shows when it refuses the item, `None` when the use goes on.
///
/// The dispatcher's order is: a container (`0x5d8d26`), a locked item (`0x5d8d32`) and an uncached
/// template (`0x5d8d5c`, silent) end it; then the gift (`0x5d8d92`) and wrapping paper arms and the
/// quest-starter (`0x5d8dcc`), which run before any gate; then the requirement check
/// (`0x5d8e0e`), ahead of the `PageText` book (`0x5d8e4c`) and the mail letter (`0x5d8e7b`); then
/// the control gate (`0x5d8eaa`, line `0x7f`), ahead of the lootable arm (`0x5d8f7c`), the charter
/// (`0x5d8f95`) and the generic use. So the requirement guards the read, letter, open and plain
/// forks and the control gate the open and plain ones.
///
/// Without control the item passes only if its on-use spell, the first block with trigger 0
/// (`0x5d8c80`), is in [`Model::control_bypass`]: a missing spell is refused too (`0x5d8ec7`,
/// `0x5d8ee7`). `known` is whether the template is cached; the reference returns silently without
/// it.
pub(crate) fn item_use_refusal(
    model: &Model,
    fork: UseFork,
    item_id: u32,
    use_spell: u32,
    known: bool,
) -> Option<&'static str> {
    if item_id == 0 || !known {
        return None;
    }
    if matches!(
        fork,
        UseFork::Equip | UseFork::Unwrap | UseFork::WrapPaper | UseFork::Quest
    ) {
        return None;
    }
    if let Some(key) = super::item_stats::item_refusal_by_id(model, item_id) {
        return Some(key);
    }
    if matches!(fork, UseFork::Read | UseFork::Letter) {
        return None;
    }
    (!model.player_control && !model.control_bypass.contains(&use_spell))
        .then_some("ERR_CLIENT_LOCKED_OUT")
}

impl super::UiScript {
    /// The dispatcher's requirement and control gates for a use the app sends itself, the bar's
    /// item (`0x4e607b`), the soulstone (`0x48ae7d`) and a confirmed bind-on-use (`0x5d92c1`), all
    /// of which call `0x5d8d00` past its quest-starter arm: the plain fork with a cached template.
    /// `item_id` 0 passes.
    pub fn item_use_refusal(&self, item_id: u32, use_spell: u32) -> Option<&'static str> {
        item_use_refusal(&self.model_ref(), UseFork::Plain, item_id, use_spell, true)
    }

    /// The on-use spells that pass the control gate right now (`0x5d8ec0`-`0x5d8f6a`), computed by
    /// the app off the caster's flags and auras; empty with control.
    pub fn set_control_bypass(&mut self, spells: std::collections::HashSet<u32>) {
        self.model_mut().control_bypass = spells;
    }
}
