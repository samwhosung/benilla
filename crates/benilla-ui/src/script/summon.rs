//! The player-summon **Era API surface** — four globals, one snapshot.
//!
//! These are the whole Lua side of being summoned, and they exist because the reference's
//! `CONFIRM_SUMMON` dialog is written against exactly them (`StaticPopup.lua:1336-1357` +
//! `UIParent.lua:551-552`): the `CONFIRM_SUMMON` event raises the dialog with **no arguments at
//! all**, `OnShow` seeds its countdown from [`GetSummonConfirmTimeLeft`], the popup engine's
//! per-tick text re-reads [`GetSummonConfirmSummoner`] and [`GetSummonConfirmAreaName`], and
//! `OnAccept` calls [`ConfirmSummon`]. `reference/1.12-globals.tsv` lists all four as
//! `function`/`engine`, and there is **no fifth**: 1.12 has no `CancelSummon`, because declining
//! sends no packet.
//!
//! **Why the three getters read a pushed snapshot rather than arguments.** The reference keeps the
//! request in four engine globals — `[0xb4e358]/[0xb4e35c]` the summoner guid, `[0xb4e354]` the
//! zone id, `[0xb4e350]` the deadline — and each getter resolves *at call time*: the summoner
//! through the name cache (`DBCache::NameCache::GetRecord 0x55f080`, `""` on a miss), the area
//! through `AreaTable.dbc` (`""` when the row is missing), the time left as
//! `(deadline − now) / 1000`. Both resolves need host state this crate does not have, so the app
//! does them and pushes the answers every frame ([`super::UiScript::set_summon_confirm`]). The
//! observable is identical, including the one that matters: a summoner whose name has not arrived
//! yet renders `""` and then fills itself in on a later tick, because the dialog re-reads the
//! getter every OnUpdate.
//!
//! `ConfirmSummon()` takes no arguments and queues only a count, [`super::binder`]'s shape and for
//! its reason: the app holds the guid that goes on the wire.

use mlua::Lua;

use super::Model;

/// The `CONFIRM_SUMMON` dialog's three reads, resolved by the app and pushed each frame.
///
/// All three are the *reference's* fallbacks when nothing is pending or nothing resolves — two
/// empty strings and a zero — because every one of its getters pushes a value on every path
/// (`""` is the shared static `0x882748`; the time-left reader returns 0 for a zeroed deadline).
/// Nothing here is ever nil, which is what lets the dialog's `format` run unguarded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SummonConfirmUiState {
    /// The summoner's resolved name, or `""` while the name query is still out.
    pub summoner: String,
    /// The summoner's zone name out of `AreaTable.dbc`, or `""` if the id names no row.
    pub area: String,
    /// Milliseconds left before the offer expires — `0` when nothing is pending or the window has
    /// already run out. **Milliseconds, not seconds**, because the reference's truncation to whole
    /// seconds happens inside the binding (`0x48b660`'s `/1000`), and that is where it happens
    /// here too: the floor is part of the API's contract, not part of the host's bookkeeping.
    pub time_left_ms: u32,
}

impl super::UiScript {
    /// Push the summon question's three resolved reads (the app's per-frame feed).
    pub fn set_summon_confirm(&mut self, state: SummonConfirmUiState) {
        let mut model = self.model_mut();
        if model.summon_confirm != state {
            model.summon_confirm = state;
        }
    }

    /// Drain the `ConfirmSummon()` calls queued since the last drain — each one is a
    /// `CMSG_SUMMON_RESPONSE`. [`Self::take_binder_confirms`]'s shape, and a count for the same
    /// reason: the intent has no payload of its own.
    pub fn take_summon_confirms(&mut self) -> u32 {
        std::mem::take(&mut self.model_mut().summon_confirms)
    }
}

/// Register the four summon globals (the style [`super::binder`] registers its two).
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetSummonConfirmSummoner() — who is summoning you (`0x48b6a0`). A string on every path.
    g.set(
        "GetSummonConfirmSummoner",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            lua.create_string(&model.summon_confirm.summoner)
        })?,
    )?;

    // GetSummonConfirmAreaName() — where to (`0x48b720`). The **summoner's** zone, straight out of
    // `AreaTable.dbc` with no parent walk and no GlobalString tail: unlike the innkeeper question's
    // three-step chain (`0x5dfe5e`), an id this table cannot name renders as `""`.
    g.set(
        "GetSummonConfirmAreaName",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            lua.create_string(&model.summon_confirm.area)
        })?,
    )?;

    // GetSummonConfirmTimeLeft() — whole SECONDS left, truncated (`0x48b660`: the `0x10624dd3`
    // magic multiply is an integer `/1000` over the millisecond remainder, then `lua_pushnumber`).
    // The dialog seeds `this.timeleft` from it in OnShow and the popup engine counts down locally
    // from there, so the floor lands once, at open.
    g.set(
        "GetSummonConfirmTimeLeft",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(f64::from(model.summon_confirm.time_left_ms / 1000))
        })?,
    )?;

    // ConfirmSummon() — the CONFIRM_SUMMON dialog's Accept, and the only packet in the whole flow.
    g.set(
        "ConfirmSummon",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.summon_confirms += 1;
            Ok(())
        })?,
    )?;

    Ok(())
}
