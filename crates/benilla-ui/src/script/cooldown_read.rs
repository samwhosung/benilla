//! The cooldown getters' query of a list inside the call. `GetActionCooldown` (`0x4e6ed0` →
//! `0x4e6ca0`), `GetSpellCooldown` (`0x4b40a0`), `GetContainerItemCooldown` (`0x4f99b0`) and
//! `GetInventoryItemCooldown` (`0x4c8a60`) ask the player's list as it stands, and
//! `GetPetActionCooldown` (`0x4bdfa0`) the pet's (`GetSpellCooldown 0x6e2ea0` and `GetItemCooldown
//! 0x6e2ed0`, both through `GetCooldownInfo 0x6e13e0`), so a listener reads its own packet's
//! records. Through the lent host when one is
//! free ([`crate::script::live::ask`]); with none, the frame's snapshot answers.

use mlua::Lua;

/// What a getter asks of a list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CooldownQuery {
    /// `0x6e2ea0` on the player's list: the spell, with item 0.
    Spell(u32),
    /// `0x6e2ed0` by entry, an action slot's item (`0x4e6cf4`): the record's first on-use spell,
    /// with the item and the record's category for that spell.
    Item(u32),
    /// `0x6e2ed0` for the item object held under the guid, a bag slot's: its entry, and a charged
    /// copy with no charge left answering nothing (`0x4f9a48`).
    ItemObject(u64),
    /// `0x6e2ed0` for the item the active player wears in descriptor slot `slot0`, the paper
    /// doll's (`0x4c8b3a`), likewise.
    Worn(u8),
    /// `0x6e2ea0` on the pet's list (`edx = 1`, `0x4be037`): a pet bar slot's spell.
    PetSpell(u32),
}

/// `(start_ms on the GetTime clock, duration_ms, enabled)`, the shape the snapshots push.
pub type CooldownTriple = (i64, u32, bool);

/// The query's own answer, `None` for its empty one: a cold spell's, or an item copy's that skips
/// the query.
impl crate::script::live::LiveQuery for CooldownQuery {
    type Answer = Option<CooldownTriple>;
}

/// `query` through the lent host, in `GetTime` seconds: `Some(None)` for the query's own empty
/// answer (a cold spell, an item whose query answers zeros), `None` with no host free to ask or
/// for an item it does not hold, where the snapshot answers.
pub(crate) fn read_live(lua: &Lua, query: CooldownQuery) -> Option<Option<(f64, f64, bool)>> {
    crate::script::live::ask(lua, &query).map(|triple| triple.map(seconds))
}

/// A pushed triple in `GetTime` seconds, as the stores keep it.
pub(crate) fn seconds((start_ms, duration_ms, enabled): CooldownTriple) -> (f64, f64, bool) {
    (
        start_ms as f64 / 1000.0,
        f64::from(duration_ms) / 1000.0,
        enabled,
    )
}
