//! The world click inside its mouse command's call ([`ScriptCall::WorldClick`]): the arm's pick
//! at the press (`ArmClick 0x514810` → `0x481f00`), and the release's dispatch (`ReleaseCommands
//! 0x514b70` → `DispatchWorldClick 0x481f60`), whose legs all run before the command returns:
//! the router calls each leg directly (`0x481fd3`, `0x481fff`, `0x482042`), each leg its select or
//! its dispatcher (`0x492d05`, `0x492d18`), and `SetSelection` signals its listeners in place. What
//! a leg leaves to a packet or to a walk's arrival is not the call's. The arm's pick needs the
//! world's geometry, which only the input records' walk lends beside the host ([`InputHost`]):
//! the mouse commands pass the movement gate only inside an input event (`0x494a50`, tag 0), and
//! the walk is where the input events run.
//!
//! A nameplate's click is the same object legs on the plate's unit, without the router
//! ([`ScriptCall::PlateClick`]): its slot (`0x7cb910`) calls `ClickObjectByAction 0x4949f0`, which
//! calls each leg directly (`0x494a22`, `0x494a39`), before the plate's own click.
//!
//! [`ScriptCall::WorldClick`]: benilla_ui::script::ScriptCall::WorldClick
//! [`ScriptCall::PlateClick`]: benilla_ui::script::ScriptCall::PlateClick

use bevy::ecs::system::SystemParam;
use bevy::prelude::Vec2;

use benilla_ui::script::{
    CallHost, CallTail, ClickCall, ClickMode, PlateAction, PlateClick, ScriptCall, UiScript,
};

use super::{disjoint, ScriptCallHost, Signalled};

/// The call host the input records' walk lends, with the world click's tracer beside it
/// ([`crate::target::ArmTracer`]), so an arm a binding or a handler makes picks inside its call
/// (`ArmClick 0x514810` → `0x51482d` → `0x481f00`). The tracer reads the meshes, the rig palettes
/// and the colliders, which no other host holds: the walk runs inside the records' exclusive
/// dispatch, so they order against nothing. Beside them `widescreen`, a CVar knob the host's
/// callbacks write, which shapes the screen the records' pointer is read on.
#[derive(SystemParam)]
pub(crate) struct InputHost<'w, 's> {
    set: disjoint::Pair<'w, 's, InputSide, Option<ScriptCallHost<'static, 'static>>>,
}

/// What the input records' walk holds beside the host: the tracer and `widescreen`.
type InputSide = (
    Option<crate::target::ArmTracer<'static, 'static>>,
    Option<bevy::prelude::Res<'static, crate::video::Widescreen>>,
);

impl InputHost<'_, '_> {
    /// `widescreen` as it stands outside any call.
    pub(crate) fn widescreen(&mut self) -> Option<crate::video::Widescreen> {
        self.set.p0().1.as_deref().copied()
    }

    /// Run `f` on `script` with the host and the tracer lent around all of it; with no host, `f`
    /// runs unlent.
    pub(crate) fn lend<R>(
        &mut self,
        script: &mut UiScript,
        f: impl FnOnce(&mut UiScript) -> R,
    ) -> R {
        if self.set.p1().is_none() {
            return f(script);
        }
        script.hosted(&mut Tracing(self), f)
    }
}

/// [`InputHost`] as the VM holds it: each call to the host, which an arm's call precedes with
/// its pick at the cursor of the record that runs ([`benilla_ui::script::ClickPress`]).
struct Tracing<'a, 'w, 's>(&'a mut InputHost<'w, 's>);

impl Tracing<'_, '_, '_> {
    fn host(&mut self) -> Option<ScriptCallHost<'_, '_>> {
        self.0.set.p1()
    }
}

impl CallHost for Tracing<'_, '_, '_> {
    fn apply(&mut self, script: &mut UiScript, call: ScriptCall) -> Option<CallTail> {
        if let ScriptCall::WorldClick(ClickCall::Arm(_)) = call {
            let at = script.click_press().at.map(Vec2::from);
            let pick = self.0.set.p0().0.map(|mut tracer| tracer.trace(at));
            #[cfg(test)]
            self.0.set.end_lend();
            if let (Some(pick), Some(mut host)) = (pick, self.host()) {
                if let Some(mut arm) = host.appliers.click().arm() {
                    arm.traced(pick);
                }
            }
        }
        self.host()?.apply(script, call)
    }

    fn read_item(
        &mut self,
        key: benilla_ui::script::ItemKey,
        read: &mut dyn FnMut(&dyn benilla_ui::script::ItemRead),
    ) -> bool {
        self.host().is_some_and(|mut h| h.read_item(key, read))
    }

    fn read_unit_object(
        &mut self,
        guid: u64,
        read: &mut dyn FnMut(&dyn benilla_ui::script::UnitObjectRead),
    ) -> bool {
        self.host()
            .is_some_and(|mut h| h.read_unit_object(guid, read))
    }

    fn read_unit_tokens(
        &mut self,
        read: &mut dyn FnMut(&dyn benilla_ui::script::UnitTokenObjects),
    ) -> bool {
        self.host().is_some_and(|mut h| h.read_unit_tokens(read))
    }

    fn read_unit_identity(
        &mut self,
        guid: u64,
        read: &mut dyn FnMut(&dyn benilla_ui::script::UnitIdentityRead),
    ) -> bool {
        self.host()
            .is_some_and(|mut h| h.read_unit_identity(guid, read))
    }

    fn read_unit_stats(
        &mut self,
        guid: u64,
        read: &mut dyn FnMut(
            &dyn benilla_ui::script::UnitObjectRead,
            &dyn benilla_ui::script::CombatRead,
        ),
    ) -> bool {
        self.host()
            .is_some_and(|mut h| h.read_unit_stats(guid, read))
    }

    fn read_own_stats(
        &mut self,
        read: &mut dyn FnMut(Option<&dyn benilla_ui::script::CombatRead>),
    ) -> bool {
        self.host().is_some_and(|mut h| h.read_own_stats(read))
    }

    fn read_self(&mut self, read: &mut dyn FnMut(&dyn benilla_ui::script::SelfRead)) -> bool {
        self.host().is_some_and(|mut h| h.read_self(read))
    }

    fn read_unit_auras(
        &mut self,
        guid: u64,
        read: &mut dyn FnMut(Option<&dyn benilla_ui::script::AuraListRead>),
    ) -> bool {
        self.host()
            .is_some_and(|mut h| h.read_unit_auras(guid, read))
    }

    fn read_live(&mut self, ask: &mut benilla_ui::script::live::LiveAsk<'_>) -> bool {
        self.host().is_some_and(|mut h| h.read_live(ask))
    }
}

impl ScriptCallHost<'_, '_> {
    /// One step of the world click, inside the mouse command's call: unless it waits for an arm's
    /// pick, which the record's world steps trace ([`crate::target::world_steps`]) when no tracer
    /// was lent ([`InputHost`]). A leg whose state a harness lacks does nothing.
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
        self.at_point(script, at);
        let leg = self
            .appliers
            .click()
            .select()
            .map(|mut select| select.cursor_leg());
        // A bind the cursor's word refused signals (`0x6e615d`) and its listeners run before the
        // click falls to `OnWorldClick`'s deselect (`0x5e03cf`).
        if leg == Some(crate::target::CursorLeg::Signalled) {
            self.at_point(script, at);
        }
        let owed = match leg {
            Some(crate::target::CursorLeg::Takes) | None => None,
            Some(_) => self
                .appliers
                .click()
                .select()
                .and_then(|mut select| select.select(script)),
        };
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
        if let Some(payload) = self.appliers.click().payload() {
            payload.right_click(script);
        }
        // The payload leg's `ClearCursor` signals in place (`0x495190` from `0x492ce0`,
        // `0x492c90`, `0x492d30`), its listeners running ahead of the interact or the walk.
        self.at_point(script, at);
        let mut legs = self.appliers.click();
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
        self.at_point(script, at);
        let owed = self
            .appliers
            .click()
            .dispatch()
            .and_then(|mut dispatch| dispatch.pay(owed));
        // A mailbox or a text object opens inside its use (`0x5f86b0` → `0x5f6820`, `0x5f58c0`).
        self.open_used(script, at);
        if let Some(owed) = owed {
            self.settle_selection(script, at, owed);
        }
    }
}

impl ScriptCallHost<'_, '_> {
    /// A nameplate's click inside its call: `0x4949f0` on the plate's unit, the left button's
    /// select or the right's select and interact, then the casts the interact's dispatchers tried
    /// (`0x5f35c0`, `0x5f06dc` → `0x6e5a90`). A unit that left since its plate was drawn, or no
    /// active player, takes nothing (`0x492619`, `0x49285c`, `0x494a13`).
    pub(super) fn plate_click(
        &mut self,
        script: &mut UiScript,
        at: &mut Signalled,
        click: PlateClick,
    ) {
        let unit = self
            .appliers
            .click()
            .plate()
            .and_then(|plate| plate.unit(click.guid));
        let Some(entity) = unit else {
            return;
        };
        match click.action {
            PlateAction::Select => self.plate_select(script, at, entity, click),
            PlateAction::Interact => self.plate_interact(script, at, entity, click),
        }
        if let Some(mut casts) = self.appliers.click().casts() {
            casts.drain(Some(script));
        }
    }

    /// `SelectObjectByClick` (`0x4925d0`): the player leg's trade (`0x492635`-`0x49278e`) and the
    /// pet's feed (`0x4927a6`-`0x4927f8`), whose signals run ahead of the rest, the greeting
    /// (`0x4927ff`), then `SelectTarget` (`0x49280c`), signalled in place.
    fn plate_select(
        &mut self,
        script: &mut UiScript,
        at: &mut Signalled,
        entity: bevy::prelude::Entity,
        click: PlateClick,
    ) {
        let mut legs = self.appliers.click();
        if let Some(mut trade) = legs.trade() {
            trade.click_on(script, entity, click.guid);
        }
        if let Some(mut feed) = legs.feed() {
            feed.click_on(script, click.guid);
        }
        self.at_point(script, at);
        let owed = self.appliers.click().plate().and_then(|mut plate| {
            plate.greet(entity);
            plate.select(entity, click.guid)
        });
        if let Some(owed) = owed {
            self.settle_selection(script, at, owed);
        }
    }

    /// `InteractWithObjectByClick` (`0x492820`): `SelectTarget` (`0x492870`), signalled in place,
    /// then the unit's interact (`0x492881`), after the bind of a unit the select handed a standing
    /// cursor has emptied the cursor's word, so the cast goes out ahead of what the interact does.
    fn plate_interact(
        &mut self,
        script: &mut UiScript,
        at: &mut Signalled,
        entity: bevy::prelude::Entity,
        click: PlateClick,
    ) {
        let owed = self
            .appliers
            .click()
            .plate()
            .and_then(|mut plate| plate.select(entity, click.guid));
        if let Some(owed) = owed {
            self.settle_selection(script, at, owed);
        }
        self.appliers.cursor().bind_parked();
        let pick = self
            .appliers
            .click()
            .plate_pick()
            .map(|pick| pick.of(entity, click.guid, click.scripted));
        let Some(pick) = pick else {
            return;
        };
        let owed = self
            .appliers
            .click()
            .dispatch()
            .and_then(|mut dispatch| dispatch.act_on(script, &pick));
        let Some(owed) = owed else {
            return;
        };
        // The leg's `SetAutoLoot` is a call of its own, which applies here, ahead of the loot or
        // the use it owes (`0x5d6c78`, `0x60bff7`, `0x5f86a6`).
        self.at_point(script, at);
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

/// A nameplate's click on `guid`'s unit through the call host, as the plate's slot applies it
/// (`0x7cb910` → `0x4949f0`), its tag 2 passed, on a fresh VM when the world holds none: for a test
/// of the click's legs. A click not `scripted` reads the press pick, as a mouse press on the plate
/// latched it.
#[cfg(test)]
pub(crate) fn click_plate(
    world: &mut bevy::prelude::World,
    guid: u64,
    action: PlateAction,
    scripted: bool,
) {
    if world.get_non_send_resource::<UiScript>().is_none() {
        super::insert_test_vm(world);
    }
    super::with_hosted_vm(world, |script| {
        script.expect("a VM").submit_plate_click(PlateClick {
            guid,
            action,
            scripted,
            may_cast: true,
        });
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
