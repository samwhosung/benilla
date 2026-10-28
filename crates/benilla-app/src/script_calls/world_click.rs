//! The world click inside its mouse command's call ([`ScriptCall::WorldClick`]): the arm's pick
//! at the press (`ArmClick 0x514810` → `0x481f00`), and the release's dispatch (`ReleaseCommands
//! 0x514b70` → `DispatchWorldClick 0x481f60`), whose legs all run before the command returns:
//! the router calls each leg directly (`0x481fd3`, `0x481fff`, `0x482042`), each leg its select or
//! its dispatcher (`0x492d05`, `0x492d18`), and `SetSelection` signals its listeners in place. What
//! a leg leaves to a packet or to a walk's arrival is not the call's.
//!
//! [`ScriptCall::WorldClick`]: benilla_ui::script::ScriptCall::WorldClick

use benilla_ui::script::{ClickCall, ClickMode, UiScript};

use super::{ScriptCallHost, Signalled};

impl ScriptCallHost<'_, '_> {
    /// One step of the world click, inside the mouse command's call: unless it waits for an arm's
    /// pick, which the record's world steps trace ([`crate::target::world_steps`]). A leg whose
    /// state a harness lacks does nothing.
    pub(super) fn world_click(
        &mut self,
        script: &mut UiScript,
        at: &mut Signalled,
        step: ClickCall,
    ) {
        let mut legs = self.appliers.click();
        let Some(mut arm) = legs.arm() else {
            return;
        };
        if arm.waits(script.has_click_calls(), step) {
            script.defer_click(step);
            return;
        }
        match step {
            ClickCall::Arm(mode) => {
                if arm.admits(mode, script.click_press()) {
                    arm.arm(mode);
                } else {
                    arm.disarm();
                }
            }
            ClickCall::Disarm => arm.disarm(),
            ClickCall::Release => {
                let Some(mode) = arm.release() else {
                    return;
                };
                // `0x514d8b` dispatches action 1 for mode 1, `0x514d77` action 4 for mode 2.
                match mode {
                    ClickMode::Select => self.select_click(script, at),
                    ClickMode::Action => self.action_click(script, at),
                }
                // The casts the click's dispatchers tried (`0x5f35c0`, `0x5f06dc` → `0x6e5a90`).
                if let Some(mut casts) = self.appliers.click().casts() {
                    casts.drain(Some(script));
                }
            }
        }
    }

    /// Action 1 on the press pick: the object leg's trade and pet feed (`0x4925d0`, ahead of its
    /// greeting and `SetSelection` at `0x49280c`), the select or the deselect, signalled in place,
    /// then the targeting cursor's terrain and object commits. A charmed player takes the nothing
    /// leg with zero points (`0x481f98`, `0x48204e`): none of them.
    fn select_click(&mut self, script: &mut UiScript, at: &mut Signalled) {
        let mut legs = self.appliers.click();
        if legs.select().is_some_and(|select| select.charmed()) {
            return;
        }
        if let Some(mut trade) = legs.trade() {
            trade.click(script);
        }
        if let Some(mut feed) = legs.feed() {
            feed.click(script);
        }
        // What the offers signalled runs ahead of the select.
        self.at_point(script, &mut at.mark);
        let owed = self
            .appliers
            .click()
            .select()
            .and_then(|mut select| select.select(script));
        if let Some(owed) = owed {
            self.settle_selection(script, at, owed);
        }
        if let Some(mut commits) = self.appliers.click().commits() {
            commits.ground();
            commits.object();
        }
    }

    /// Action 4 on the press pick: the payload leg, then for an object the INTERACT leg's select
    /// (`0x492870`), signalled in place and bound to a standing cursor's cast by `SelectTarget`
    /// (`0x4935d5`), and its dispatcher (`0x492881`); for the ground or the sky, the walk.
    fn action_click(&mut self, script: &mut UiScript, at: &mut Signalled) {
        let mut legs = self.appliers.click();
        if let Some(payload) = legs.payload() {
            payload.right_click(script);
        }
        let Some(mut dispatch) = legs.dispatch() else {
            return;
        };
        // A charmed player takes the nothing leg with zero points (`0x481f98`, `0x48204e`).
        if dispatch.charmed() {
            return;
        }
        let owed = dispatch.select();
        if let Some(owed) = owed {
            self.settle_selection(script, at, owed);
        }
        self.appliers.cursor().bind_parked();
        let owed = self
            .appliers
            .click()
            .dispatch()
            .and_then(|mut dispatch| dispatch.act(script));
        let Some(owed) = owed else {
            return;
        };
        // The leg's `SetAutoLoot` is a call of its own, which applies here, ahead of the loot or
        // the use it owes (`0x5d6c78`, `0x60bff7`, `0x5f86a6`).
        self.at_point(script, &mut at.mark);
        let owed = self
            .appliers
            .click()
            .dispatch()
            .and_then(|mut dispatch| dispatch.pay(owed));
        if let Some(owed) = owed {
            self.settle_selection(script, at, owed);
        }
    }
}

/// Apply the world click steps waiting (made with no host lent, or after an arm that found no
/// press's pick) through the call host, in the order made. Whether any waited.
pub(crate) fn submit_waiting_clicks(world: &mut bevy::prelude::World) -> bool {
    let waiting = world
        .get_non_send_resource::<UiScript>()
        .is_some_and(|s| s.has_click_calls());
    if waiting {
        super::with_hosted_vm(world, |script| {
            if let Some(script) = script {
                script.submit_queued_clicks();
            }
        });
    }
    waiting
}

/// A clean release of an armed `mode` click on the press pick as it stands, through the call host,
/// as a mouse command's release applies it, on a fresh VM when the world holds none: for a test of
/// the click's legs.
#[cfg(test)]
pub(crate) fn release_click(world: &mut bevy::prelude::World, mode: ClickMode) {
    if world.get_non_send_resource::<UiScript>().is_none() {
        super::insert_test_vm(world);
    }
    let now = world.resource::<bevy::prelude::Time>().elapsed_secs();
    world
        .resource_mut::<crate::target::ClickArm>()
        .arm_at(mode, now);
    super::with_hosted_vm(world, |script| {
        script
            .expect("a VM")
            .submit_click(benilla_ui::script::ClickCall::Release);
    });
}

/// Every catalog line defined as its own key in the world's VM, and a recorder of the lines
/// `UI_ERROR_MESSAGE` shows in `ERROR_LINES`, put in once: for a test of a refusal the click's call
/// shows inside it, as `DisplayError` (`0x496720`) signals before it returns.
#[cfg(test)]
pub(crate) fn record_error_lines(world: &mut bevy::prelude::World) {
    if world.get_non_send_resource::<UiScript>().is_none() {
        super::insert_test_vm(world);
    }
    let script = world.non_send_resource::<UiScript>();
    if script
        .eval::<bool>("return ERROR_LINES ~= nil")
        .unwrap_or(false)
    {
        return;
    }
    let mut lua = String::new();
    for key in benilla_ui::messages::CATALOG.iter().map(|r| r.key) {
        if !key.is_empty() {
            lua.push_str(&format!("{key} = \"{key}\"\n"));
        }
    }
    lua.push_str(
        r#"ERROR_LINES = {}
        local r = CreateFrame("Frame")
        r:RegisterEvent("UI_ERROR_MESSAGE")
        r:SetScript("OnEvent", function() table.insert(ERROR_LINES, arg1) end)"#,
    );
    script.run(&lua).expect("the recorder installs");
}

/// The catalog keys of the lines shown since [`record_error_lines`], in the order shown.
#[cfg(test)]
pub(crate) fn error_lines(world: &bevy::prelude::World) -> Vec<&'static str> {
    let Some(script) = world.get_non_send_resource::<UiScript>() else {
        return Vec::new();
    };
    let lines: Vec<String> = script.eval("return ERROR_LINES or {}").unwrap_or_default();
    lines
        .iter()
        .filter_map(|line| {
            benilla_ui::messages::CATALOG
                .iter()
                .map(|r| r.key)
                .find(|key| key == line)
        })
        .collect()
}

/// [`error_lines`], the record emptied after.
#[cfg(test)]
pub(crate) fn take_error_lines(world: &bevy::prelude::World) -> Vec<&'static str> {
    let keys = error_lines(world);
    if let Some(script) = world.get_non_send_resource::<UiScript>() {
        let _ = script.run("if ERROR_LINES then ERROR_LINES = {} end");
    }
    keys
}
