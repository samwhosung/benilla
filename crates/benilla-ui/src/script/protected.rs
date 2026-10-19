//! The protected-action gate `0x494a50(tag)`: whether a script may run an action reserved for the
//! player's own input. Whose code runs ([`taint`]) and which input event is running decide.
//!
//! A nonzero context maps the tag through the byte table `0x494b54` to one of three arms of the
//! jump table `0x494b48` ([`ARMS`]): *refuse* (tags 0 and 11) is refused whatever input event runs
//! and signals `MACRO_ACTION_FORBIDDEN` for a `RunScript` chunk or `ADDON_ACTION_FORBIDDEN` with the
//! addon's name (`0x494ab8`-`0x494afb`), once at a time ([`Flags::refusing`]); *input* (tags 2, 4 and
//! 10) passes only while an input event runs and is otherwise refused silently (`0x494a92`); every
//! other tag passes (`0x494b11`). Context 0, the client's own code, passes every tag but 0 and 11,
//! which pass only while an input event runs (`0x494b1a`).
//!
//! The three input events are the flags the gate reads, each raised around exactly the code the
//! reference raises it around:
//! - [`Event::Enter`], `[0xcf4dc4]` (`0x77e430`): an edit box's `OnEnterPressed` and its listener
//!   notify (`0x77b438`-`0x77b459`), Enter in a single-line box only;
//! - [`Event::Click`], `[0xcf4da0]` (`0x779890`): a button's click slot `+0x94` or `+0x98` run by the
//!   mouse, which is `OnClick` or `OnDoubleClick`, on a press with a Down registration
//!   (`0x779275`-`0x77928b`) and on a release (`0x779352`-`0x7793b8`). Not `OnMouseDown` or
//!   `OnMouseUp`, which run before the flag is raised (`0x76bf70`, `0x76c040`), and not `Click()`,
//!   which calls the slot with no flag (`0x782826`);
//! - [`Event::Binding`], `[[0xb71290]+0xd8]` (`0x494aa4`): `CBindings::ExecuteBinding` (`0x4b7990`)
//!   running a key's body (`0x4b7b02`-`0x4b7b11`).
//!
//! Each is a store of 1 and later 0, never a count, so an event raised inside the same one ends it
//! early in the reference; the flags here count, which only differs where the reference's own code
//! could not nest them (the three writers are the key, mouse and Enter handlers, none reachable from
//! another's script but through a Lua call, which raises nothing).
//!
//! The gate is asked at the Lua verb, where the stack scan is true. A call the app applies later
//! ([`crate::script::ScriptCall`]) carries the answer for the tag it needs (`may_cast`, tag 2;
//! `may_pick`, tag 4), and a macro's lines, which the app runs inside `UseAction`, replay the
//! events they were pressed under ([`InputEvents`]).

use mlua::Lua;

use super::taint::{self, Context};
use super::tick::fire_event_into;
use super::{Model, ScriptValue, UiScript};

/// A gate tag, the `ecx` every caller loads before `call 0x494a50`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Tag(pub u8);

impl Tag {
    /// The movement functions (23 callers, `input_verbs`).
    pub(crate) const MOVEMENT: Tag = Tag(0);
    /// `TryCastSpell 0x6e4b60`, `BindSpellTargetObject 0x6e5b40`, `PressPetAction 0x4bd1d0`,
    /// `SpellTargetUnit 0x6e6d90`, `SpellStopTargeting 0x6e6e30`, `SpellStopCasting 0x6e6e80`,
    /// `TogglePetSlotAutocast 0x4bcbb0`, `SendSpellAutocast 0x4bccb0`, `SendPetAbandon 0x4bd740`.
    pub(crate) const CAST: Tag = Tag(2);
    /// `CGGameUI::TargetNearest 0x493f60`.
    pub(crate) const TARGET: Tag = Tag(4);
    /// `CGGameUI::Reload 0x491380`.
    pub(crate) const RELOAD: Tag = Tag(10);
}

/// An arm of the jump table `0x494b48`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Arm {
    /// `0x494ab8`.
    Refuse,
    /// `0x494a92`.
    Input,
    /// `0x494b11`.
    Pass,
}

/// The byte table `0x494b54` read through the jump table `0x494b48`: `[0, 2, 1, 2, 1, 2, 2, 2, 2, 2,
/// 1, 0]` with `0` refuse, `1` input and `2` pass. A tag past 11 passes (`0x494a7e ja 0x494b11`).
const ARMS: [Arm; 12] = [
    Arm::Refuse,
    Arm::Pass,
    Arm::Input,
    Arm::Pass,
    Arm::Input,
    Arm::Pass,
    Arm::Pass,
    Arm::Pass,
    Arm::Pass,
    Arm::Pass,
    Arm::Input,
    Arm::Refuse,
];

/// The three input events the gate reads, as raised right now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputEvents {
    /// `[[0xb71290]+0xd8]`: a key binding's body is running.
    pub binding: bool,
    /// `[0xcf4dc4]`: an edit box's enter script is running.
    pub enter: bool,
    /// `[0xcf4da0]`: a button's click, run by the mouse, is running.
    pub click: bool,
}

impl InputEvents {
    /// No input event: a call from a timer, an event handler or the network.
    pub const NONE: InputEvents = InputEvents {
        binding: false,
        enter: false,
        click: false,
    };

    /// Whether any of the three is raised: what `0x494a92` and `0x494b1a` test.
    pub fn any(self) -> bool {
        self.binding || self.enter || self.click
    }
}

/// One of the three input events.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    Binding,
    Enter,
    Click,
}

/// The VM's half of the gate: how many of each input event are running, and `[0xb4e3e4]`.
#[derive(Clone, Default)]
pub(crate) struct Flags {
    binding: u32,
    enter: u32,
    click: u32,
    /// `[0xb4e3e4]`: set while a refusal's event is being signalled, so a handler that calls a
    /// gated function in turn is refused without a second event (`0x494abf`, `0x494acc`).
    refusing: bool,
}

impl Flags {
    fn counter(&mut self, event: Event) -> &mut u32 {
        match event {
            Event::Binding => &mut self.binding,
            Event::Enter => &mut self.enter,
            Event::Click => &mut self.click,
        }
    }

    pub(crate) fn events(&self) -> InputEvents {
        InputEvents {
            binding: self.binding > 0,
            enter: self.enter > 0,
            click: self.click > 0,
        }
    }
}

/// An input event raised until the guard drops, so it ends on every path out of the code it wraps.
#[must_use]
pub(crate) struct Raised {
    lua: Lua,
    event: Event,
}

impl Raised {
    fn end(&self) {
        if let Some(mut model) = self.lua.app_data_mut::<Model>() {
            let counter = model.protected.counter(self.event);
            *counter = counter.saturating_sub(1);
        }
    }
}

impl Drop for Raised {
    fn drop(&mut self) {
        self.end();
    }
}

/// Raise `event` until the returned guard drops.
pub(crate) fn raise(lua: &Lua, event: Event) -> Raised {
    if let Some(mut model) = lua.app_data_mut::<Model>() {
        *model.protected.counter(event) += 1;
    }
    Raised {
        lua: lua.clone(),
        event,
    }
}

/// The input events running now.
pub(crate) fn events(lua: &Lua) -> InputEvents {
    lua.app_data_ref::<Model>()
        .expect("model app_data")
        .protected
        .events()
}

/// The gate's answer before any signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    Allow,
    /// `xor eax,eax; ret` with nothing signalled.
    Deny,
    /// `0x494ab8`: refused, and the context's event signalled.
    Refuse,
}

fn decide(context: &Context, events: InputEvents, tag: Tag) -> Verdict {
    let input = |events: InputEvents| {
        if events.any() {
            Verdict::Allow
        } else {
            Verdict::Deny
        }
    };
    let arm = ARMS.get(usize::from(tag.0)).copied().unwrap_or(Arm::Pass);
    match context {
        // `0x494a5a je 0x494b11`: only the two refuse-arm tags test the events (`0x494b11`).
        Context::Client => match arm {
            Arm::Refuse => input(events),
            Arm::Input | Arm::Pass => Verdict::Allow,
        },
        _ => match arm {
            Arm::Refuse => Verdict::Refuse,
            Arm::Input => input(events),
            Arm::Pass => Verdict::Allow,
        },
    }
}

/// The gate `0x494a50(tag)` for the running call: whether it may go on. A refuse-arm tag in a
/// nonzero context signals its event; the other refusals are silent.
pub(crate) fn gate(lua: &Lua, tag: Tag) -> bool {
    let context = taint::current(lua);
    match decide(&context, events(lua), tag) {
        Verdict::Allow => true,
        Verdict::Deny => false,
        Verdict::Refuse => {
            refuse(lua, &context);
            false
        }
    }
}

/// The gate's tag 2 for a verb that selects a unit. `SelectTarget` (`0x493540`) asks it only inside
/// the bind it makes while the targeting cursor is up (`0x4935d5` → `0x6e5ba5`), and tag 2 refuses
/// silently, so asking at every such verb shows nothing when the cursor is down. The cursor may go up
/// between the verb and the app's application of its call, so the answer is carried either way.
pub(crate) fn select_may_cast(lua: &Lua) -> bool {
    gate(lua, Tag::CAST)
}

/// The refusal arm `0x494ab8`: `MACRO_ACTION_FORBIDDEN` (`0x1af`) for a `RunScript` chunk's
/// context, else `ADDON_ACTION_FORBIDDEN` (`0x1b0`) with the context's text, signalled in place.
fn refuse(lua: &Lua, context: &Context) {
    {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        if std::mem::replace(&mut model.protected.refusing, true) {
            return;
        }
    }
    match context {
        Context::Macro => fire_event_into(lua, "MACRO_ACTION_FORBIDDEN", Vec::new()),
        Context::Addon(name) => fire_event_into(
            lua,
            "ADDON_ACTION_FORBIDDEN",
            vec![ScriptValue::Str(name.clone())],
        ),
        Context::Client => {}
    }
    lua.app_data_mut::<Model>()
        .expect("model app_data")
        .protected
        .refusing = false;
}

impl UiScript {
    /// Note that the call applied under `events` moved the selection to `guid`. The reference
    /// signals `PLAYER_TARGET_CHANGED` inside `SetSelection`, so inside the key or click that made
    /// the call; the app's unit feed signals it when it sees the change, so it asks for these
    /// ([`Self::take_selection_cause`]) and raises them around that event.
    pub fn note_selection_cause(&mut self, guid: Option<u64>, events: InputEvents) {
        self.model_mut().selection_cause = Some((guid, events));
    }

    /// The events the selection change to `guid` was made under, if a call noted one; none
    /// otherwise (a click, the server). Spent by the asking.
    pub fn take_selection_cause(&mut self, guid: Option<u64>) -> InputEvents {
        match self.model_mut().selection_cause.take() {
            Some((cause, events)) if cause == guid => events,
            _ => InputEvents::default(),
        }
    }

    /// The input events running now, for a call the app applies after they end.
    pub fn input_events(&self) -> InputEvents {
        events(self.lua())
    }

    /// Run `f` with exactly `events` raised, those running now lowered meanwhile: a call applied
    /// after its verb returned, its macro lines and what it signals run under the events the verb
    /// ran under, as in the reference, which does the call inside the verb; never under those of
    /// the entry that applies it.
    pub fn with_input_events<R>(
        &mut self,
        events: InputEvents,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let _exactly = Exactly::raise(self.lua(), events);
        f(self)
    }
}

/// The input events set to exactly a call's until the guard drops, then put back as they were, so
/// they end on every path out of the code it wraps.
#[must_use]
struct Exactly {
    lua: Lua,
    before: (u32, u32, u32),
}

impl Exactly {
    fn raise(lua: &Lua, events: InputEvents) -> Exactly {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        let flags = &mut model.protected;
        let before = (flags.binding, flags.enter, flags.click);
        (flags.binding, flags.enter, flags.click) = (
            u32::from(events.binding),
            u32::from(events.enter),
            u32::from(events.click),
        );
        Exactly {
            lua: lua.clone(),
            before,
        }
    }
}

impl Drop for Exactly {
    fn drop(&mut self) {
        if let Some(mut model) = self.lua.app_data_mut::<Model>() {
            let flags = &mut model.protected;
            (flags.binding, flags.enter, flags.click) = self.before;
        }
    }
}
