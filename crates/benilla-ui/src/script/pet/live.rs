//! The pet getters' questions at the call ([`super::super::live`]): the bar's guid, slot words,
//! mode word and attack latch (`0xb714a0`, `0xb71438`, `0xb71468`, `0xb714b0`), the pet the guid
//! resolves and the pet's book (`0xb6f098`), each read inside its binding (`0x4bdc50`,
//! `0x4bcf70`, `0x4be670`, `0x4be500`). With no host to ask, each answers off the copy the VM
//! holds.

use std::borrow::Cow;

use mlua::Lua;

use super::super::live::{ask, LiveQuery};
use super::super::{Model, PetBookState};
use super::{PetActionView, PetStats, StoredPetAction};

/// The bar's guid-level answers as they stand: `PetHasActionBar` (the guid, `0x4bdc20`),
/// `GetPetActionsUsable` (`0x4bcf70`), `PickupPetAction`'s gate (`0x4be1f7`, `0x4be20a`) and
/// `GetPetTimeRemaining`'s time to the expiry tick `0xb714a8`, read off the clock at the call
/// (`0x4be614`).
#[derive(Clone, Copy, Debug)]
pub struct PetBarFlags;

impl LiveQuery for PetBarFlags {
    type Answer = PetBarFlagsView;
}

/// [`PetBarFlags`]' answer.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PetBarFlagsView {
    pub has_bar: bool,
    pub actions_usable: bool,
    pub pickup_allowed: bool,
    /// The milliseconds from now to the expiry, negative past it; `None` for no expiry, as after
    /// a packet whose duration is 0.
    pub remaining_ms: Option<i64>,
}

/// One slot, 0-based, as `GetPetActionInfo` paints it now (`0x4bdc50`): `None` with no guid or
/// past the ten (`0x4bdc9d`-`0x4bdcbf`). Its cooldown is unread: `GetPetActionCooldown` asks the
/// pet's list ([`super::super::CooldownQuery::PetSpell`]).
#[derive(Clone, Copy, Debug)]
pub struct PetSlot(pub usize);

impl LiveQuery for PetSlot {
    type Answer = Option<PetActionView>;
}

/// The ten slots as they stand, empty with no guid, which the drag's core walks (`0x4bc9a0`).
#[derive(Clone, Copy, Debug)]
pub struct PetSlots;

impl LiveQuery for PetSlots {
    type Answer = Vec<PetActionView>;
}

/// `HasPetUI`'s first return and the stat block now, off the pet the guid resolves
/// (`0x4be670`, the gate `0x6116e0`).
#[derive(Clone, Copy, Debug)]
pub struct PetStatsNow;

impl LiveQuery for PetStatsNow {
    type Answer = (bool, PetStats);
}

/// `(PetCanBeAbandoned, PetCanBeRenamed)` now (`0x4be500`, `0x4be580`).
#[derive(Clone, Copy, Debug)]
pub struct PetMenuNow;

impl LiveQuery for PetMenuNow {
    type Answer = (bool, bool);
}

/// The pet's book as it stands (`0xb6f098`, count `0xb71174`), and `HasPetSpells`' token.
#[derive(Clone, Copy, Debug)]
pub struct PetBookNow;

impl LiveQuery for PetBookNow {
    type Answer = PetBookState;
}

/// The bar's guid-level answers now: the host's, else the copy's.
pub(crate) fn flags_now(lua: &Lua) -> PetBarFlagsView {
    ask(lua, &PetBarFlags).unwrap_or_else(|| {
        let now = crate::script::clock::now(lua);
        let bar = &lua.app_data_ref::<Model>().expect("model app_data").pet_bar;
        PetBarFlagsView {
            has_bar: bar.has_bar,
            actions_usable: bar.actions_usable,
            pickup_allowed: bar.pickup_allowed,
            #[allow(clippy::cast_possible_truncation)] // a tick difference
            remaining_ms: bar.expiry.map(|e| ((e - now) * 1000.0).round() as i64),
        }
    })
}

/// The slot at 1-based button `i` now: the host's, else the copy's.
pub(crate) fn slot_now(lua: &Lua, i: u32) -> Option<PetActionView> {
    let n = usize::try_from(i.checked_sub(1)?).ok()?;
    match ask(lua, &PetSlot(n)) {
        Some(slot) => slot,
        None => lua
            .app_data_ref::<Model>()
            .expect("model app_data")
            .pet_bar
            .slots
            .get(n)
            .map(|s| s.view.clone()),
    }
}

/// Seat the copy's bar from the host's, for a verb that runs the bar's own core on the words as
/// they stand (the drag `0x4bc9a0`, the autocast flip `0x4bcbb0`); the copy as it is with no host.
pub(crate) fn seat_bar(lua: &Lua) {
    let (Some(flags), Some(slots)) = (ask(lua, &PetBarFlags), ask(lua, &PetSlots)) else {
        return;
    };
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let bar = &mut model.pet_bar;
    bar.has_bar = flags.has_bar;
    bar.actions_usable = flags.actions_usable;
    bar.pickup_allowed = flags.pickup_allowed;
    bar.slots = slots
        .into_iter()
        .map(|view| StoredPetAction {
            view,
            cooldown: None,
        })
        .collect();
}

/// `HasPetUI`'s first return and the stat block now: the host's, else the copy's.
pub(crate) fn stats_now(lua: &Lua) -> (bool, PetStats) {
    ask(lua, &PetStatsNow).unwrap_or_else(|| {
        let bar = &lua.app_data_ref::<Model>().expect("model app_data").pet_bar;
        (bar.has_ui, bar.stats.clone())
    })
}

/// The menu's two predicates now: the host's, else the copy's.
pub(crate) fn menu_now(lua: &Lua) -> (bool, bool) {
    ask(lua, &PetMenuNow).unwrap_or_else(|| {
        let bar = &lua.app_data_ref::<Model>().expect("model app_data").pet_bar;
        (bar.can_be_abandoned, bar.can_be_renamed)
    })
}

/// The pet's book now: the host's, else the copy's.
pub(crate) fn book_now<'a>(lua: &Lua, model: &'a Model) -> Cow<'a, PetBookState> {
    match ask(lua, &PetBookNow) {
        Some(book) => Cow::Owned(book),
        None => Cow::Borrowed(&model.pet_book),
    }
}
