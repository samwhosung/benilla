//! The session verbs: the game menu's Logout and Exit Game, their dialogs' answers, `ReloadUI`
//! and the cinematic pair. Each queues a [`SessionRequest`] the app turns into a packet or an
//! exit; the server decides whether a logout is instant or a 20-second countdown. Beside them,
//! the world-enter cascade's two events and the `PLAYER_LOGIN` arm a UI load sets.

use mlua::Lua;

use super::Model;

/// A session intent queued by a Lua call, drained by the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionRequest {
    /// `Logout()`: `CMSG_LOGOUT_REQUEST` (`0x5ab000`), back to character select.
    Logout,
    /// `Quit()`: the same request, then the process ends once the logout completes.
    Quit,
    /// `CancelLogout()`: `CMSG_LOGOUT_CANCEL`; the server's ack fires `LOGOUT_CANCEL`.
    CancelLogout,
    /// `ForceQuit()`: end the process now, with no server round trip.
    ForceQuit,
    /// `ForceLogout()` (`0x48ab50`): an empty `CMSG_PLAYER_LOGOUT` (`0x4A`), bypassing the
    /// pending-logout latch, and nothing without a live in-world session (`0x5ab020`).
    ForceLogout,
    /// `ReloadUI()` (`0x4884d0`) only sets `0xb4b3f4`; the next frame's `0x495590` tears the VM
    /// down and rebuilds it, so it is never destroyed mid-call. The app's drain, outside any call,
    /// is the same guard.
    ReloadUi,
    /// `StopCinematic()` (`0x48b970`), the only skip: no native caller and no binding, so ESC
    /// reaches it only through `CinematicFrame`'s `OnKeyDown`; playback therefore waits for the
    /// in-game UI.
    StopCinematic,
}

impl super::UiScript {
    /// Drain the session intents queued since the last call.
    pub fn take_session_requests(&mut self) -> Vec<SessionRequest> {
        std::mem::take(&mut self.model_mut().session_requests)
    }

    /// Queue an intent from the app: `/logout`, `/camp`, `/quit` and `/reload`, which benilla
    /// parses in Rust rather than through `SlashCmdList`, take their Lua verbs' route this way.
    pub fn queue_session_request(&mut self, request: SessionRequest) {
        self.model_mut().session_requests.push(request);
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

    /// The world-enter cascade's events (`0x4908c0`): a full collection
    /// (`FrameScriptCollectGarbage(1)`, `0x490946`), then `PLAYER_LOGIN` (270, `0x490959`) only if
    /// a load armed it (read at `0x49094b`, cleared at `0x49095e`), then `PLAYER_ENTERING_WORLD`
    /// (272, `0x49096a`) every time, then the minimap's zoom reseed
    /// ([`Self::minimap_enter_world`]), the bags' `BAG_UPDATE`s ([`Self::bags_enter_world`]), the
    /// quest log's rebuild and its `QUEST_LOG_UPDATE`
    /// ([`Self::quest_log_enter_world`]), the reputation list's rebuild and its `UPDATE_FACTION`
    /// ([`Self::reputation_enter_world`]) and the skill list's rebuild and its
    /// `SKILL_LINES_CHANGED` ([`Self::rebuild_skills`]). The cascade runs with the player object in
    /// the world, so its caller seats `"player"` first, and inside its own sound-suppression
    /// bracket (`0x4908d5` to `0x490a56`), so a handler's `PlaySound` is dropped.
    pub fn fire_world_enter(&mut self) {
        let armed = std::mem::take(&mut self.model_mut().player_login_armed);
        self.push_sound_suppression();
        // `0x703bc0` with 1: the threshold set to 0 (`0x703bd6`), so the collection runs now.
        self.lua.set_gc_threshold_kb(0);
        if armed {
            self.fire_event("PLAYER_LOGIN", Vec::new());
        }
        self.fire_event("PLAYER_ENTERING_WORLD", Vec::new());
        // The calls after the signal, inside the bracket: the minimap initializer first
        // (`0x49096f`), then among the rest the bag watchers' set-up (`0x49098d`), the quest log's
        // (`0x4909a1`), the reputation rebuild (`0x4909d3`) and the skill list's (`0x4909f1` →
        // `0x4d2c90`).
        self.minimap_enter_world();
        self.bags_enter_world();
        self.quest_log_enter_world();
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
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                model.session_requests.push(request);
                Ok(())
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
