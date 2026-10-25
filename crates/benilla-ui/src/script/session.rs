//! The session verbs: the game menu's Logout and Exit Game, their dialogs' answers, `ReloadUI`
//! and the cinematic pair. Each hands its [`SessionRequest`] to the call host, which sends the
//! packet, sets the latch or ends the process before the verb returns, as the reference's do
//! (`Logout` `0x489390` → `0x5ab000`); the server decides whether a logout is instant or a
//! 20-second countdown. Beside them, the world-enter cascade's two events and the `PLAYER_LOGIN`
//! arm a UI load sets.

use mlua::Lua;

use super::Model;

/// A session verb, which the call host applies inside the call ([`super::ScriptCall::Session`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionRequest {
    /// `Logout()` (`0x489390`): `0x5ab000(0, 0)`, `CMSG_LOGOUT_REQUEST`, back to character select.
    Logout,
    /// `Quit()` (`0x4893b0`): `0x5ab000(1, 0)`, the same request, then the process ends once the
    /// logout completes.
    Quit,
    /// `CancelLogout()` (`0x48ab40` → `0x5aaf80`): `CMSG_LOGOUT_CANCEL`, and the pending latch
    /// clears.
    CancelLogout,
    /// `ForceQuit()` (`0x48ab60` → `0x401560(0)`): end the process now, with no server round
    /// trip.
    ForceQuit,
    /// `ForceLogout()` (`0x48ab50` → `0x5aaff0`): `0x5ab000(0, 1)`, an empty `CMSG_PLAYER_LOGOUT`
    /// (`0x4A`), bypassing the pending-logout latch, and nothing without a live in-world session
    /// (`0x5ab020`).
    ForceLogout,
    /// `ReloadUI()` (`0x4884d0`) only sets `0xb4b3f4`; the next frame's `0x495590` tears the VM
    /// down and rebuilds it, so it is never destroyed mid-call.
    ReloadUi,
    /// `StopCinematic()` (`0x48b970` → `0x48f050`), the only skip: no native caller and no
    /// binding, so ESC reaches it only through `CinematicFrame`'s `OnKeyDown`; playback therefore
    /// waits for the in-game UI.
    StopCinematic,
}

impl super::UiScript {
    /// Hand `request` to the call host, as its Lua verb does: `/logout`, `/camp`, `/quit` and
    /// `/reload`, which benilla parses in Rust rather than through `SlashCmdList`, and the
    /// engine's own calls (the idle handler's `0x5ab000`, the console's reload), take this route.
    /// What the call raises, a call made with no host lent among it, is the error.
    pub fn session_call(&mut self, request: SessionRequest) -> mlua::Result<()> {
        let lua = self.lua().clone();
        super::calls::submit(&lua, super::ScriptCall::Session(request))
    }

    /// The session calls a harness VM kept ([`Self::keep_calls`]), in call order.
    pub fn take_session_requests(&mut self) -> Vec<SessionRequest> {
        self.take_calls_where(|call| match call {
            super::ScriptCall::Session(request) => Some(*request),
            _ => None,
        })
    }

    /// Arm `PLAYER_LOGIN`, the UI load's last step: `UI_Init` sets the one-shot `[0xb4e260]` at
    /// `0x49011d`, unconditionally, and [`Self::fire_world_enter`] spends it. The flag lives in
    /// this VM, so each load arms its own and a worldport, which loads nothing, re-arms nothing.
    pub fn arm_player_login(&mut self) {
        self.model_mut().player_login_armed = true;
    }

    /// The bag watchers' set-up in the world-enter cascade (`0x4f8cc0`): the bag cache zeroed,
    /// then rescanned (`0x4f8da6` → `0x4f91a0`), so each of bags 1 to 10 the player holds fires
    /// `BAG_UPDATE` with its number, in order (`0x4f9343`); the backpack, the bank and the keyring
    /// are no bags of the cache. A bag held is a container pushed for its number.
    fn bags_enter_world(&mut self) {
        let held: Vec<i64> = {
            let model = self.model_ref();
            (1..=10)
                .filter(|n| model.containers.contains_key(n))
                .collect()
        };
        for n in held {
            self.fire_event("BAG_UPDATE", vec![super::ScriptValue::Int(n)]);
        }
    }

    /// [`Self::fire_world_enter_with`] at the bonus bar offset the VM holds, for a harness with no
    /// player form to read.
    pub fn fire_world_enter(&mut self) {
        let offset = self.model_ref().bonus_bar_offset;
        self.fire_world_enter_with(offset);
    }

    /// The world-enter cascade's events (`0x4908c0`): a full collection
    /// (`FrameScriptCollectGarbage(1)`, `0x490946`), then `PLAYER_LOGIN` (270, `0x490959`) only if
    /// a load armed it (read at `0x49094b`, cleared at `0x49095e`), then `PLAYER_ENTERING_WORLD`
    /// (272, `0x49096a`) every time, then the minimap's zoom reseed
    /// ([`Self::minimap_enter_world`]), the action bar's usable refresh and its
    /// `ACTIONBAR_UPDATE_USABLE` when the caller says a pair changed, the bonus bar's offset,
    /// `bonus_bar_offset`, from the player's form and its `UPDATE_BONUS_ACTIONBAR`, the bags'
    /// `BAG_UPDATE`s ([`Self::bags_enter_world`]), the quest log's rebuild and its `QUEST_LOG_UPDATE`
    /// ([`Self::quest_log_enter_world`]), the pet bar's reset ([`Self::drop_pet_bar`],
    /// [`Self::fire_pet_bar_dropped`]), the reputation list's rebuild and its `UPDATE_FACTION`
    /// ([`Self::reputation_enter_world`]) and the skill list's rebuild and its
    /// `SKILL_LINES_CHANGED` ([`Self::rebuild_skills`]). The cascade runs with the player object in
    /// the world, so its caller seats `"player"` first, and inside its own sound-suppression
    /// bracket (`0x4908d5` to `0x490a56`), so a handler's `PlaySound` is dropped.
    ///
    /// The pet bar here is the VM's own: a caller that holds the bar's guid runs the cascade in
    /// its three parts, [`Self::fire_world_enter_head`], the reset on both sides, then
    /// [`Self::fire_world_enter_tail`].
    pub fn fire_world_enter_with(&mut self, bonus_bar_offset: u8) {
        self.fire_world_enter_head(bonus_bar_offset, false);
        let dropped = self.drop_pet_bar();
        self.fire_pet_bar_dropped(dropped);
        self.fire_world_enter_tail();
    }

    /// The cascade up to the pet bar's reset (`0x4909c9`): the bracket opened, the collection,
    /// `PLAYER_LOGIN` if armed, `PLAYER_ENTERING_WORLD` and the calls after it through the quest
    /// log's (`0x4909a1`). `usable_changed` is whether the action bar's refresh found a slot's pair
    /// changed, which the caller's slots decide. [`Self::fire_world_enter_tail`] closes it.
    pub fn fire_world_enter_head(&mut self, bonus_bar_offset: u8, usable_changed: bool) {
        self.seat_reputation();
        let armed = std::mem::take(&mut self.model_mut().player_login_armed);
        self.push_sound_suppression();
        // `0x703bc0` with 1: the threshold set to 0 (`0x703bd6`), so the collection runs now.
        self.lua.set_gc_threshold_kb(0);
        if armed {
            self.fire_event("PLAYER_LOGIN", Vec::new());
        }
        self.fire_event("PLAYER_ENTERING_WORLD", Vec::new());
        // The calls after the signal, inside the bracket: the minimap initializer first
        // (`0x49096f`), then among the rest the action bar's refresh (`0x490979` → `0x4e4f90`):
        // the usable refresh `0x4e5c00`, signalling `ACTIONBAR_UPDATE_USABLE` on a changed pair
        // (`0x4e5c53`), then the bonus bar (`0x4e4f95` → `0x4e4fc0`), which sets the offset and
        // signals whether or not it moved (`0x4e5018`); then the bag watchers' set-up
        // (`0x49098d`) and the quest log's (`0x4909a1`).
        self.minimap_enter_world();
        if usable_changed {
            self.fire_event("ACTIONBAR_UPDATE_USABLE", Vec::new());
        }
        self.set_bonus_bar_offset(bonus_bar_offset);
        self.fire_event("UPDATE_BONUS_ACTIONBAR", Vec::new());
        self.bags_enter_world();
        self.quest_log_enter_world();
    }

    /// The cascade after the pet bar's reset: the reputation rebuild (`0x4909d3`) and the skill
    /// list's (`0x4909f1` → `0x4d2c90`), then the bracket closed (`0x490a56`).
    pub fn fire_world_enter_tail(&mut self) {
        self.reputation_enter_world();
        self.rebuild_skills();
        self.pop_sound_suppression();
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    for (name, request) in [
        ("Logout", SessionRequest::Logout),
        ("Quit", SessionRequest::Quit),
        ("CancelLogout", SessionRequest::CancelLogout),
        ("ForceQuit", SessionRequest::ForceQuit),
        ("ForceLogout", SessionRequest::ForceLogout),
        ("ReloadUI", SessionRequest::ReloadUi),
        ("StopCinematic", SessionRequest::StopCinematic),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, ()| {
                // `ReloadUI` (`0x4884d0`) sets the reload byte only if `CGGameUI::Reload`'s gate
                // (`0x491380`, tag 10) allows: a script's own call needs an input event, and is
                // otherwise ignored with no message.
                if request == SessionRequest::ReloadUi
                    && !super::protected::gate(lua, super::protected::Tag::RELOAD)
                {
                    return Ok(());
                }
                super::calls::submit(lua, super::ScriptCall::Session(request))
            })?,
        )?;
    }

    // `0x48c930`: the number 1 while a cinematic plays, else nil, one result either way.
    g.set(
        "InCinematic",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(if model.in_cinematic {
                mlua::Value::Integer(1)
            } else {
                mlua::Value::Nil
            })
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::{ContainerState, UiScript};

    /// The cascade's bag watcher set-up (`0x49098d` → `0x4f8cc0`) zeroes the bag cache and
    /// rescans it (`0x4f8da6` → `0x4f91a0`): `BAG_UPDATE` for each of the ten bags present, in
    /// order, after `PLAYER_ENTERING_WORLD`; the backpack, the bank and the keyring are no bags of
    /// the cache.
    #[test]
    fn the_world_enter_cascade_updates_each_bag_present() {
        let mut s = UiScript::new().unwrap();
        for bag in [-2, -1, 0, 1, 3, 5, 10] {
            s.set_container(
                bag,
                Some(ContainerState {
                    name: None,
                    num_slots: 4,
                    slots: Default::default(),
                }),
            );
        }
        s.run(
            "Heard = {} local f = CreateFrame('Frame') f:RegisterEvent('BAG_UPDATE') \
             f:RegisterEvent('PLAYER_ENTERING_WORLD') \
             f:SetScript('OnEvent', function() table.insert(Heard, event .. ':' .. tostring(arg1)) end)",
        )
        .unwrap();
        s.fire_world_enter();
        assert_eq!(
            s.eval::<Vec<String>>("return Heard").unwrap(),
            [
                "PLAYER_ENTERING_WORLD:nil",
                "BAG_UPDATE:1",
                "BAG_UPDATE:3",
                "BAG_UPDATE:5",
                "BAG_UPDATE:10"
            ]
        );
    }
}
