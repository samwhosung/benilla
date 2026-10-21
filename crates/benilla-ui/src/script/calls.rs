//! The script calls whose effects touch the selection, the player's cast or the targeting cursor,
//! each applied inside its verb through the lent host ([`super::UiScript::hosted`]). In the
//! reference each such call is done when it returns: the macro runner `0x4f14e0` fires every line
//! in one pass, `TargetUnit` (`0x4899d0`), `TargetByName` (`0x489d60`) and `ClearTarget`
//! (`0x489ff0`) commit through `SetSelection 0x493540` before returning, and `CastSpellByName`
//! (`0x4b4ab0`) casts at the selection as it stands then. So `/target Bob` then `/cast Flash Heal`
//! heals Bob.
//!
//! A verb hands its call to [`submit`], which applies it before the verb returns. While the host is
//! out (applying another call, or in an engine turn, [`super::UiScript::host_out`]) the handlers
//! the engine fires are held for it ([`super::host::holding`]), so their calls apply in place once
//! they run; a call made meanwhile by Lua the host ran in place (a chunk) waits. Both run as soon
//! as the host is back, in the order made, ahead of any made later. With no host lent a call is
//! refused; a harness with no engine keeps it instead ([`super::UiScript::keep_calls`]).
//!
//! A call that selects a unit may find the spell targeting cursor up when it is applied:
//! `SelectTarget` (`0x493540`) then binds the unit to the pending cast and returns without
//! selecting (`0x4935d5`), behind the gate's tag 2 inside `BindSpellTargetObject` (`0x6e5ba5`). Each
//! such call carries that answer as `may_cast`, asked where the Lua verb ran.

use std::collections::VecDeque;

use mlua::Lua;

use super::{ActionUse, Model, ScriptValue, SelectionRequest, UiScript};

/// One script call a verb hands the host, in the reference's terms.
#[derive(Clone, Debug, PartialEq)]
pub enum ScriptCall {
    /// `TargetUnit`, `AssistUnit`, `AssistByName` and `TargetLastEnemy`.
    Select {
        request: SelectionRequest,
        /// The gate's tag 2, which `SelectTarget`'s bind asks while the cursor is up (`0x6e5ba5`).
        may_cast: bool,
    },
    /// `TargetByName(name, exactMatch)` (`0x489d60`), the stock `/target`.
    TargetByName {
        name: String,
        exact: bool,
        /// As [`ScriptCall::Select::may_cast`].
        may_cast: bool,
    },
    /// `TargetNearestEnemy`, `TargetNearestFriend`, `TargetNearestPartyMember` and
    /// `TargetNearestRaidMember` (`0x489a80`/`0x489aa0`/`0x489ac0`/`0x489ae0`): one TAB cycle,
    /// `0x493f60(reverse, mode)`.
    TargetNearest {
        mode: NearestMode,
        reverse: bool,
        /// As [`ScriptCall::Select::may_cast`], for the pick's `SelectTarget` (`0x4941e6`); tag 4
        /// opened the cycle (`0x493f9b`).
        may_cast: bool,
    },
    /// `TargetLastTarget()` (`0x489b00`): re-select the last-target pair, or deselect when it is
    /// empty.
    TargetLastTarget {
        /// As [`ScriptCall::Select::may_cast`]; the empty pair's deselect asks no gate (`0x4935ac`).
        may_cast: bool,
    },
    /// `ClearTarget()` (`0x489ff0` → `0x493540(0,0)`), a no-op when nothing is selected by then.
    ClearTarget,
    /// `AttackTarget()` (`0x489b50` → `0x6131a0(0,0)`): the attack validator's pick from the
    /// selection as it stands then, which may move it to the nearest enemy, then the toggle.
    AttackTarget {
        /// The gate's tag 4 allowed the pick (`0x6130bc` → `0x493f9b`): with none, the validator
        /// takes the selection as it stands.
        may_pick: bool,
        /// As [`ScriptCall::Select::may_cast`], for the `SelectTarget` of a kept unit
        /// (`0x613100`) and of the cycle's pick.
        may_cast: bool,
    },
    /// `SpellTargetUnit(unit)` (`0x6e6d90`), made while the cursor was up.
    SpellTargetUnit(String),
    /// `SpellStopTargeting()` (`0x6e6e30`), made while the cursor was up.
    SpellStopTargeting,
    /// `SpellStopCasting()` (`0x6e6e80`), made with something to stop.
    SpellStopCasting,
    /// `CastSpell(id, "spell", onSelf)` and `CastSpellByName(name, onSelf)` on a player-book name,
    /// through `0x4b3300`'s player leg; `onSelf` swaps the target guid for the active player's
    /// (`0x4b4afa`, `0x4b4345`).
    CastSpell {
        spell_id: u32,
        on_self: bool,
        /// The gate's tag 2 allowed the cast (`0x6e4bc1`). The book leg's cancels (an active aura
        /// `0x4b3466`, a form `0x4b35da`) come before it, so a refusal stops only the cast.
        may_cast: bool,
    },
    /// `CastSpell(id, "pet", onSelf)` and `CastSpellByName(name, onSelf)` on a name only the pet's
    /// book holds (`0x4b3b51`): `0x4b3300`'s `CMSG_PET_ACTION` fork (`0x4b34ce`), with the same
    /// guid swap.
    CastPetSpell { spell_id: u32, on_self: bool },
    /// `CastShapeshiftForm(index)` (`0x4b4810`), as the form's spell.
    CastShapeshiftForm {
        spell_id: u32,
        /// The gate's tag 2 allowed the cast (`0x4b4992` → `0x6e4bc1`); the cancels at `0x4b496b`
        /// and `0x4b497c` come before it.
        may_cast: bool,
    },
    /// `UseAction(slot, …)` (`0x4e7140` → `0x4e5ee0`); a macro slot runs its lines inside it.
    UseAction(ActionUse),
    /// `UseContainerItem(bag, slot, onSelf)` outside repair mode, its arm chosen and locked in the
    /// call.
    UseContainerItem(super::ContainerUse),
    /// `UseInventoryItem(slot, onSelf)` outside repair mode: `on_self` makes the player the
    /// use's target in place of the selection (`0x4c8e37`-`0x4c8e48`).
    UseInventoryItem {
        slot: u32,
        on_self: bool,
        /// The gate's tag 2 allowed the cast ([`super::ContainerUse::may_cast`]).
        may_cast: bool,
    },
    /// `CastPetAction(slot)`, a 1-based pet bar slot.
    PetAction(u32),
    /// `PetAttack` and the other one-shot orders, as their bar slot's packed word.
    PetOrder(u32),
    /// `SendChatMessage(text, type, language, target)` (`0x49f1e0`) past the binding's checks: the
    /// host runs the rest of the verb inside the call, from the group gates to the packet, and
    /// raises its escape error there ([`UiScript::raise_in_call`]).
    SendChat(super::ChatSend),
    /// `RandomRoll(min, max)` (`0x48c7b0`) past its range check (`0x48c807`-`0x48c830`):
    /// `MSG_RANDOM_ROLL` (`0x48c832`).
    RandomRoll { min: u32, max: u32 },
    /// `DoEmote(token, target)` (`0x49fd30`) past its empty-token check: the host finds the
    /// `EmotesText` row by name (`0x49fd72`-`0x49fda6`), resolves the target (`0x49fdb1`-`0x49fdee`)
    /// and runs `0x5ef560` (`0x49fe16`), its gates, posture, play and `CMSG_TEXT_EMOTE`.
    DoEmote(super::EmoteRequest),
    /// `UninviteByName(name)` (`0x48a610`): `CMSG_GROUP_UNINVITE` (`0x75`), sent at `0x48a669`.
    Uninvite(String),
    /// `SendAddonMessage(prefix, message, type)` (`0x49f920`) past its checks: the send at
    /// `0x49faff`.
    AddonMessage(super::AddonSend),
    /// A channel verb past its binding's checks, or the recruitment cascade's join or leave: the
    /// host does the rest inside the call, the join's slot (`0x49ede8`-`0x49edf3`, the allocator
    /// `0x49b980`) and the packet, so the verb's next line and the events after it read the slot.
    Channel(super::ChannelCommand),
    /// A bag or paper-doll click while targeting wants an item (`PickupContainerItem` at
    /// `0x4f9c54`, `PickupInventoryItem` through `0x4c7300` at `0x4c76df`): the item seam
    /// `ValidateSpellItemTarget 0x495d60` on the clicked slot, which binds and sends, raises a
    /// confirm, or refuses before the verb returns.
    PickItem(super::ItemPick),
    /// `BindEnchant()` (`0x48d2e0`) or `ReplaceEnchant()` (`0x48d300`): a confirm popup's Yes on
    /// the item the seam parked.
    EnchantConfirm(super::EnchantAnswer),
    /// A loot window verb, a row's own click among them ([`super::LootCall`]).
    Loot(super::LootCall),
    /// An auction verb past its binding's checks: its gates, refusals and packet
    /// ([`super::AuctionCall`]).
    Auction(super::AuctionCall),
    /// A friend or ignore verb ([`super::SocialCall`]): its packet, its selection and its line
    /// inside the call.
    Social(super::SocialCall),
    /// A party or raid verb past its binding's checks ([`super::PartyRequest`]): the host sends
    /// its packet inside the call, so a chunk's group packets go out in call order with every
    /// other verb's.
    Party(super::PartyRequest),
    /// A mail verb past its binding's guards: its packet, and for `GetInboxText` the read mark
    /// and the body's ask ([`super::MailCall`]).
    Mail(super::MailCall),
}

/// The mode the four `TargetNearest*` shims hand the TAB cycler `0x493f60`, the only byte that
/// differs between them; it reaches only the per-candidate filter `0x493e40`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NearestMode {
    /// Mode 1, `TargetNearestEnemy`.
    Enemy,
    /// Mode 2, `TargetNearestFriend`.
    Friend,
    /// Mode 3, `TargetNearestPartyMember`: a party member other than us.
    PartyMember,
    /// Mode 4, `TargetNearestRaidMember`: a party or raid member other than us.
    RaidMember,
}

impl ScriptCall {
    /// What the protected-action gate's tag 2 allowed the verb that queued this call, for the casts
    /// applying it starts (`TryCastSpell`, `0x6e4bc1`) or the bind of a selection it makes while
    /// the targeting cursor is up (`0x6e5ba5`); a call that does neither, or that was refused at
    /// the verb and never queued, reads passed.
    pub fn may_cast(&self) -> bool {
        match self {
            ScriptCall::UseAction(press) => press.may_cast,
            ScriptCall::UseContainerItem(item) => item.may_cast,
            ScriptCall::UseInventoryItem { may_cast, .. } => *may_cast,
            ScriptCall::PickItem(pick) => pick.may_cast,
            ScriptCall::EnchantConfirm(answer) => answer.may_cast,
            ScriptCall::CastSpell { may_cast, .. } => *may_cast,
            ScriptCall::CastShapeshiftForm { may_cast, .. } => *may_cast,
            ScriptCall::Select { may_cast, .. }
            | ScriptCall::TargetByName { may_cast, .. }
            | ScriptCall::TargetNearest { may_cast, .. }
            | ScriptCall::TargetLastTarget { may_cast }
            | ScriptCall::AttackTarget { may_cast, .. } => *may_cast,
            _ => true,
        }
    }
}

/// A call and the input events running when its verb made it. The reference does the call inside
/// that verb, so what it signals runs with those events up ([`super::protected`]); a call that
/// waited for the host gets them raised again around it.
#[derive(Clone, Debug, PartialEq)]
pub struct QueuedCall {
    pub call: ScriptCall,
    pub input: super::InputEvents,
    /// The call's number, under which a raise it makes is filed for its verb.
    seq: u64,
}

/// The raises the host makes in its calls' verbs ([`UiScript::raise_in_call`]).
#[derive(Clone, Debug, Default)]
pub(crate) struct CallRaises {
    /// The number the next queued call takes.
    next: u64,
    /// The calls whose verbs are inside [`submit`], waiting for them to apply.
    awaited: Vec<u64>,
    /// What the call applying now raised.
    raising: Option<String>,
    /// The raises of awaited calls, for their verbs to take.
    raised: Vec<(u64, String)>,
}

/// What waits for the host, in the order made: a call, or a handler an engine step fired while
/// the host was out ([`Held`]), each with the input events running when it was made, and the lock
/// and cursor writes made among them ([`Waiting::Lock`], [`Waiting::Cursor`]).
#[derive(Clone, Debug)]
pub(crate) enum Waiting {
    Call(QueuedCall),
    Lua {
        held: Held,
        input: super::InputEvents,
        /// The `PlaySound` suppression depth it was fired under (`0x458f50`): a load's or a
        /// sweep's handlers stay silent though they run once the scope has closed.
        muted: u32,
    },
    /// A write of item `guid`'s lock bit made while something waited ([`Model::set_item_lock`]):
    /// `locked` after it, `was` before it. The take puts the bit back ([`take_waiting`]) and the
    /// write is made again in its place, so what waited ahead of it reads the lock as it stood
    /// then, as `UnlockItem` (`0x495420`) signals before the step's next write.
    Lock {
        guid: u64,
        locked: bool,
        was: bool,
    },
    /// A cursor write made while something waited ([`super::cursor::write`]): the take puts the
    /// cursor back as it found it, so what waited ahead of it reads the cursor as it stood then,
    /// and its operation runs again in its place against the cursor as it is then, as
    /// `ClearCursor` (`0x495190`) acts on what the cursor holds when it runs. `shown`: Lua the
    /// turn ran in place made it and its events fired then, so it is made again without them.
    Cursor {
        write: Box<super::cursor::CursorWrite>,
        shown: bool,
    },
}

/// A fire held for the host ([`super::host::holding`]): run as it would have run then.
#[derive(Clone, Debug)]
pub(crate) enum Held {
    /// An event's fan-out to its listeners (`SignalEvent`, `0x703e50`).
    Event(String, Vec<ScriptValue>),
    /// A verb segment's fan-out ([`super::tick::verb`]): its events, the cursor views around
    /// them, its lock writes and its cursor writes, in order, so each handler reads the locks as
    /// its event left them, and each cursor write acts on the cursor as it then is.
    Segment(Vec<super::tick::Raised>),
    /// One widget script (`OnShow`, `OnValueChanged`, …) on frame `id`, with its `arg1..argN`.
    Handler {
        id: u32,
        script: String,
        extra: Vec<mlua::Value>,
    },
}

impl super::Model {
    /// Hold `call` for the host, with the input events running now; its number.
    fn queue_call(&mut self, call: ScriptCall) -> u64 {
        let input = self.protected.events();
        let seq = self.call_raises.next;
        self.call_raises.next += 1;
        self.push_waiting(Waiting::Call(QueuedCall { call, input, seq }));
        seq
    }

    /// Put `item` behind what waits.
    pub(crate) fn push_waiting(&mut self, item: Waiting) {
        self.script_calls.push(item);
    }
}

/// Hold `held` for the host, with the input events running now: an engine step fired it while it
/// could not lend the host ([`super::host::holding`]).
pub(crate) fn hold(lua: &Lua, held: Held) {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let input = model.protected.events();
    let muted = model.sound_suppression;
    model.push_waiting(Waiting::Lua { held, input, muted });
}

/// Take what waits, with the lock bits and the cursor its writes changed put back, last first:
/// each item then runs against the locks and the cursor as they stood when it was made, and each
/// write is made again in its place ([`Waiting::Lock`], [`Waiting::Cursor`], a held segment's
/// own).
fn take_waiting(lua: &Lua) -> VecDeque<Waiting> {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let model = &mut *model;
    let batch = std::mem::take(&mut model.script_calls);
    for item in batch.iter().rev() {
        match item {
            Waiting::Lock { guid, was, .. } => {
                super::tick::set_lock_bit(model, *guid, *was);
            }
            Waiting::Cursor { write, .. } => write.rewind(model),
            Waiting::Lua {
                held: Held::Segment(raised),
                ..
            } => super::tick::rewind(model, raised),
            Waiting::Lua { .. } | Waiting::Call(_) => {}
        }
    }
    batch.into()
}

/// Make a lock write that waited, in its place.
fn relock(lua: &Lua, guid: u64, locked: bool) {
    lua.app_data_mut::<Model>()
        .expect("model app_data")
        .set_item_lock(guid, locked);
}

/// Make a cursor write that waited again, in its place, against the cursor as it is now: as the
/// engine half made it, outside any segment, or with `shown` silently, its events having fired
/// as it was first made.
fn rewrite(lua: &Lua, write: Box<super::cursor::CursorWrite>, shown: bool) {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    if shown {
        let outer = model.verb_events.replace(Vec::new());
        super::cursor::write(&mut model, write.op);
        model.verb_events = outer;
    } else {
        super::cursor::write(&mut model, write.op);
    }
}

/// Run a held fire now, under the input events and the sound suppression it was fired under.
fn replay(lua: &Lua, script: &mut UiScript, held: Held, input: super::InputEvents, muted: u32) {
    let outer = std::mem::replace(
        &mut lua
            .app_data_mut::<Model>()
            .expect("model app_data")
            .sound_suppression,
        muted,
    );
    script.with_input_events(input, |_| run_held(lua, held));
    lua.app_data_mut::<Model>()
        .expect("model app_data")
        .sound_suppression = outer;
}

/// Run `held` now, as it would have run when it was fired.
fn run_held(lua: &Lua, held: Held) {
    match held {
        Held::Event(event, args) => super::tick::fire_event_into(lua, &event, args),
        Held::Segment(raised) => super::tick::fan_out(lua, raised, super::tick::Fan::Again),
        Held::Handler { id, script, extra } => {
            if let Err(e) = super::event::fire_widget_handler(lua, id, &script, extra) {
                lua.app_data_mut::<Model>()
                    .expect("model app_data")
                    .record_lua_error(&e);
            }
        }
    }
}

/// How deep calls made while applying a call may nest: a macro whose line presses a macro, and so
/// on. A call made at the ninth level (a macro that presses itself) is not applied: its verb
/// raises [`TOO_DEEP`] in the Lua that made it, which reports it as that script's error, and the
/// eight calls under it stand applied. So such a chain always ends inside its frame. The
/// reference's own bound on it is not read.
pub(crate) const MAX_NESTING: u8 = 8;

/// What a verb raises when no host is lent: an entry ran Lua without lending one.
pub(crate) const NO_HOST: &str = "no call host is lent for this script call";

/// What a verb raises past [`MAX_NESTING`].
pub(crate) const TOO_DEEP: &str = "script calls nested too deep";

/// A verb's call: with a host lent and free, applied before the verb returns, after whatever was
/// waiting for it, under the input events running now; with the host out, held until it is back.
/// What the host raises in the call ([`UiScript::raise_in_call`]) is the verb's error. The caller
/// holds no borrow of the model.
pub(crate) fn submit(lua: &Lua, call: ScriptCall) -> mlua::Result<()> {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    if !super::host::open(lua) {
        return match model.kept_calls.as_mut() {
            Some(kept) => {
                kept.push(call);
                Ok(())
            }
            None => Err(mlua::Error::runtime(NO_HOST)),
        };
    }
    if model.call_depth >= MAX_NESTING {
        return Err(mlua::Error::runtime(TOO_DEEP));
    }
    let seq = model.queue_call(call);
    model.call_raises.awaited.push(seq);
    drop(model);
    /// The verb stops waiting for its call as it returns or unwinds.
    struct Awaited<'a>(&'a Lua, u64);
    impl Drop for Awaited<'_> {
        fn drop(&mut self) {
            if let Some(mut model) = self.0.app_data_mut::<Model>() {
                model.call_raises.awaited.retain(|&s| s != self.1);
            }
        }
    }
    let awaited = Awaited(lua, seq);
    drain(lua);
    drop(awaited);
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let raised = &mut model.call_raises.raised;
    match raised.iter().position(|&(s, _)| s == seq) {
        Some(at) => Err(mlua::Error::runtime(raised.remove(at).1)),
        None => Ok(()),
    }
}

/// File what the call `seq` raised: for its verb, while that waits inside [`submit`]; else, its
/// verb having returned while the call waited for the host, as its script's error, which the
/// error handler hears as it would the verb's raise.
fn file_raise(lua: &Lua, seq: u64, message: String) {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    if model.call_raises.awaited.contains(&seq) {
        model.call_raises.raised.push((seq, message));
    } else {
        // The reference never defers a call: its verb raises in its own body, so the lines after
        // it never run and a `pcall` around it catches the error. A call that waited for the host
        // had its verb return without an error, the script's later lines ran, and a `pcall` saw
        // nothing; the error reaches the handler only now, after them.
        model.record_lua_error(&mlua::Error::runtime(message));
    }
}

/// Swap the applying call's raise for `raise`, returning what was there.
fn swap_raising(lua: &Lua, raise: Option<String>) -> Option<String> {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    std::mem::replace(&mut model.call_raises.raising, raise)
}

/// Run what waits through the lent host, front to back: each call and its tail under exactly the
/// input events its verb ran under, each held handler under those it was fired under. What an
/// applied call left while its host was out (the handlers it fired, a call made meanwhile) runs
/// next, as the host comes back and before its tail, ahead of the rest; the calls its tail makes
/// apply inside the tail.
pub(crate) fn drain(lua: &Lua) {
    if !super::host::free(lua) {
        return;
    }
    {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        if model.script_calls.is_empty() {
            return;
        }
        model.call_depth += 1;
    }
    let mut batch = take_waiting(lua);
    struct Depth<'a>(&'a Lua);
    impl Drop for Depth<'_> {
        fn drop(&mut self) {
            if let Some(mut model) = self.0.app_data_mut::<Model>() {
                model.call_depth = model.call_depth.saturating_sub(1);
            }
        }
    }
    let _depth = Depth(lua);
    let mut script = UiScript::reentrant(lua);
    while !batch.is_empty() {
        if !super::host::free(lua) {
            // A tail took the host out: the rest waits for it, ahead of what was made since.
            requeue(lua, batch);
            return;
        }
        let Some(item) = batch.pop_front() else {
            break;
        };
        match item {
            Waiting::Call(QueuedCall { call, input, seq }) => {
                // What the host raises while it applies the call, settles what the call left and
                // runs its tail is this call's.
                let outer = swap_raising(lua, None);
                // The reference does the call inside the verb, so what it signals runs inside the
                // key, click or Enter that made it, and only inside that: a queued call gets
                // exactly its events.
                let tail = script.with_input_events(input, |script| {
                    super::host::with_host(lua, |host| host.apply(script, call)).flatten()
                });
                settle(lua, &mut script);
                if let Some(tail) = tail {
                    script.with_input_events(input, tail);
                }
                if let Some(message) = swap_raising(lua, outer) {
                    file_raise(lua, seq, message);
                }
            }
            Waiting::Lua { held, input, muted } => replay(lua, &mut script, held, input, muted),
            Waiting::Lock { guid, locked, .. } => relock(lua, guid, locked),
            Waiting::Cursor { write, shown } => rewrite(lua, write, shown),
        }
        drain(lua);
    }
}

/// Run what an applied call left while its host was out, now that it is back: the handlers it
/// fired at this call's level, so a call one makes is the call's own nested call, and a call made
/// meanwhile one level in. The reference runs both inside the call.
fn settle(lua: &Lua, script: &mut UiScript) {
    let mut left = take_waiting(lua);
    while let Some(item) = left.pop_front() {
        if !super::host::free(lua) {
            left.push_front(item);
            requeue(lua, left);
            return;
        }
        match item {
            Waiting::Lua { held, input, muted } => replay(lua, script, held, input, muted),
            Waiting::Lock { guid, locked, .. } => relock(lua, guid, locked),
            Waiting::Cursor { write, shown } => rewrite(lua, write, shown),
            call @ Waiting::Call(_) => {
                requeue(lua, [call]);
                drain(lua);
            }
        }
    }
}

/// Put `calls` back at the front of the queue, ahead of any queued since.
fn requeue(lua: &Lua, calls: impl IntoIterator<Item = Waiting>) {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let later = std::mem::replace(&mut model.script_calls, calls.into_iter().collect());
    model.script_calls.extend(later);
}

impl super::Model {
    /// Queue an engine-side refusal for the app to show, with the input events running now.
    pub(crate) fn push_ui_error(&mut self, key: &'static str) {
        let input = self.protected.events();
        self.ui_errors.push((key, input));
    }
}

/// Settle what waits for a host that will not come back (an engine turn's leftovers with no host
/// to lend, or a lend's once the outermost lend has ended): the held handlers run, with the calls
/// they make waiting, and every call is dropped. The count of calls dropped.
pub(crate) fn refuse_waiting(lua: &Lua) -> usize {
    super::host::open_unheld(lua, || {
        let mut dropped = 0;
        loop {
            let batch = take_waiting(lua);
            if batch.is_empty() {
                return dropped;
            }
            let mut script = UiScript::reentrant(lua);
            for item in batch {
                match item {
                    Waiting::Call(_) => dropped += 1,
                    Waiting::Lua { held, input, muted } => {
                        replay(lua, &mut script, held, input, muted)
                    }
                    Waiting::Lock { guid, locked, .. } => relock(lua, guid, locked),
                    Waiting::Cursor { write, shown } => rewrite(lua, write, shown),
                }
            }
        }
    })
}

impl UiScript {
    /// Raise `message` in the verb of the call the host is applying, as that verb returns: the
    /// reference's verb raises in its own body (`lua_error`, through `0x6f4940`), after what it did
    /// before, which stands. A verb that returned while its call waited for the host reports it as
    /// its script's error. For [`super::CallHost::apply`] and the tail it leaves.
    pub fn raise_in_call(&mut self, message: &str) {
        self.model_mut().call_raises.raising = Some(message.to_string());
    }

    /// Make this VM a harness's: a call its verbs make with no host lent is kept, for the
    /// `take_*` calls to read, in place of the refusal. For a VM with no engine behind it.
    pub fn keep_calls(&mut self) {
        let mut model = self.model_mut();
        if model.kept_calls.is_none() {
            model.kept_calls = Some(Vec::new());
        }
    }

    /// Take every call [`Self::keep_calls`] kept, in call order.
    pub fn take_script_calls(&mut self) -> Vec<ScriptCall> {
        self.model_mut()
            .kept_calls
            .as_mut()
            .map(std::mem::take)
            .unwrap_or_default()
    }

    /// Take the kept calls `pick` answers for, in call order, leaving the rest kept.
    pub(crate) fn take_calls_where<T>(
        &mut self,
        mut pick: impl FnMut(&ScriptCall) -> Option<T>,
    ) -> Vec<T> {
        let mut model = self.model_mut();
        let mut taken = Vec::new();
        if let Some(kept) = model.kept_calls.as_mut() {
            kept.retain(|call| match pick(call) {
                Some(t) => {
                    taken.push(t);
                    false
                }
                None => true,
            });
        }
        taken
    }
}

#[cfg(test)]
mod tests {
    use super::super::host::{CallHost, CallTail};
    use super::super::InputEvents;
    use super::*;

    /// A host that records each call it applies, and for the first one runs `inner` with the host
    /// out of the VM, as an applier's engine half runs a handler.
    struct Nesting {
        applied: Vec<ScriptCall>,
        inner: Option<&'static str>,
    }

    impl CallHost for Nesting {
        fn apply(&mut self, script: &mut UiScript, call: ScriptCall) -> Option<CallTail> {
            self.applied.push(call);
            if let Some(chunk) = self.inner.take() {
                script.run(chunk).expect("the inner chunk runs");
            }
            None
        }
    }

    fn select(token: &str) -> ScriptCall {
        ScriptCall::Select {
            request: SelectionRequest::Unit(token.into()),
            may_cast: true,
        }
    }

    const LAST_ENEMY: ScriptCall = ScriptCall::Select {
        request: SelectionRequest::LastEnemy,
        may_cast: true,
    };

    /// With the host out applying a call, a call its handler makes waits, and applies before the
    /// next call the script makes.
    #[test]
    fn a_call_made_while_the_host_is_out_applies_next_in_call_order() {
        let mut s = UiScript::new().unwrap();
        let mut host = Nesting {
            applied: Vec::new(),
            inner: Some("TargetLastEnemy()"),
        };
        s.hosted(&mut host, |s| {
            s.run(r#"TargetUnit("player") ClearTarget()"#).unwrap()
        });
        assert_eq!(
            host.applied,
            vec![select("player"), LAST_ENEMY, ScriptCall::ClearTarget]
        );
    }

    /// A host that records each call and leaves `tail` to run with the host back, noting whether
    /// the tail's own calls were applied by the time it ended.
    struct Tailing {
        applied: Vec<ScriptCall>,
        tail: Option<&'static str>,
        forever: bool,
        errors: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
    }

    impl CallHost for Tailing {
        fn apply(&mut self, _: &mut UiScript, call: ScriptCall) -> Option<CallTail> {
            self.applied.push(call);
            let chunk = if self.forever {
                self.tail
            } else {
                self.tail.take()
            }?;
            let errors = self.errors.clone();
            Some(Box::new(move |script: &mut UiScript| {
                if let Err(e) = script.run(chunk) {
                    errors.borrow_mut().push(e.to_string());
                }
            }))
        }
    }

    fn tailing(tail: &'static str, forever: bool) -> Tailing {
        Tailing {
            applied: Vec::new(),
            tail: Some(tail),
            forever,
            errors: Default::default(),
        }
    }

    /// A tail runs with the host back in the VM, so the calls it makes apply inside it, ahead of
    /// the calls the script makes after the one that left it.
    #[test]
    fn a_tails_calls_apply_inside_it() {
        let mut s = UiScript::new().unwrap();
        let mut host = tailing("TargetLastEnemy()", false);
        s.hosted(&mut host, |s| {
            s.run(r#"TargetUnit("player") ClearTarget()"#).unwrap()
        });
        assert_eq!(
            host.applied,
            vec![select("player"), LAST_ENEMY, ScriptCall::ClearTarget]
        );
        assert!(host.errors.borrow().is_empty());
    }

    /// A call that makes itself again (a macro pressing its own button) nests [`MAX_NESTING`]
    /// deep, and the verb past the bound raises, so a frame always ends and nothing waits after.
    #[test]
    fn a_call_that_makes_itself_nests_to_the_bound_then_raises() {
        let mut s = UiScript::new().unwrap();
        let mut host = tailing("TargetLastEnemy()", true);
        s.hosted(&mut host, |s| s.run("TargetLastEnemy()").unwrap());
        assert_eq!(host.applied.len(), usize::from(MAX_NESTING));
        let errors = host.errors.borrow();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains(TOO_DEEP), "{errors:?}");
        drop(errors);
        assert_eq!(s.refuse_waiting(), 0, "nothing waits");
    }

    /// With no host lent a call is refused with an error: an entry ran Lua without lending one.
    #[test]
    fn with_no_host_a_call_is_refused() {
        let mut s = UiScript::new().unwrap();
        let e = s.run("ClearTarget()").unwrap_err().to_string();
        assert!(e.contains(NO_HOST), "{e}");
        s.keep_calls();
        s.run("ClearTarget()").unwrap();
        assert_eq!(
            s.take_script_calls(),
            vec![ScriptCall::ClearTarget],
            "a harness's VM keeps it"
        );
    }

    /// A host that records each call and raises in the verb of every `ClearTarget` it applies,
    /// after running `inner` with the host out, as an applier's engine half runs a handler.
    #[derive(Default)]
    struct Raising {
        applied: Vec<ScriptCall>,
        inner: Option<&'static str>,
    }

    impl CallHost for Raising {
        fn apply(&mut self, script: &mut UiScript, call: ScriptCall) -> Option<CallTail> {
            let raises = call == ScriptCall::ClearTarget;
            self.applied.push(call);
            if let Some(chunk) = self.inner.take() {
                script.run(chunk).expect("the inner chunk runs");
            }
            if raises {
                script.raise_in_call("refused in the call");
            }
            None
        }
    }

    /// The host's raise reaches the verb of the call it applied as that verb returns, as the
    /// reference's verb raises in its own body (`0x49f769`, `SendChatMessage`'s escape error): the
    /// call stands applied, the script's next line does not run, and `pcall` catches it.
    #[test]
    fn a_raise_in_the_call_reaches_its_verb_after_what_the_call_did() {
        let mut s = UiScript::new().unwrap();
        let mut host = Raising::default();
        let e = s
            .hosted(&mut host, |s| s.run("ClearTarget() TargetLastEnemy()"))
            .unwrap_err()
            .to_string();
        assert!(e.contains("refused in the call"), "{e}");
        assert_eq!(host.applied, vec![ScriptCall::ClearTarget]);
        let (ok, e) = s
            .hosted(&mut host, |s| {
                s.eval::<(bool, String)>("local ok, e = pcall(ClearTarget) return ok, tostring(e)")
            })
            .unwrap();
        assert!(!ok && e.contains("refused in the call"), "{e}");
        s.hosted(&mut host, |s| s.run("TargetLastEnemy()"))
            .expect("a call that raises nothing returns");
        assert!(s.take_errors().is_empty());
    }

    /// A call that waited for the host applies after its verb returned, so its raise is that
    /// script's error, which the error handler hears; the call that was applying raises nothing.
    #[test]
    fn a_raise_in_a_call_that_waited_is_its_scripts_error() {
        let mut s = UiScript::new().unwrap();
        s.run("HEARD = {} seterrorhandler(function(e) table.insert(HEARD, tostring(e)) end)")
            .unwrap();
        let mut host = Raising {
            inner: Some("ClearTarget()"),
            ..Default::default()
        };
        s.hosted(&mut host, |s| s.run("TargetLastEnemy()"))
            .expect("the outer verb's own call raised nothing");
        assert_eq!(host.applied, vec![LAST_ENEMY, ScriptCall::ClearTarget]);
        s.dispatch_script_errors_to_handler();
        let heard: Vec<String> = s.eval("return HEARD").expect("the handler's record");
        assert_eq!(heard.len(), 1, "{heard:?}");
        assert!(heard[0].contains("refused in the call"), "{heard:?}");
    }

    /// A key's input event raised alone.
    const KEY: InputEvents = InputEvents {
        binding: true,
        ..InputEvents::NONE
    };

    /// A host that records the input events each call applies under, and those its tail runs
    /// under.
    #[derive(Default)]
    struct Events {
        applied: Vec<(ScriptCall, InputEvents)>,
        tails: std::rc::Rc<std::cell::RefCell<Vec<InputEvents>>>,
    }

    impl CallHost for Events {
        fn apply(&mut self, script: &mut UiScript, call: ScriptCall) -> Option<CallTail> {
            self.applied.push((call, script.input_events()));
            let tails = self.tails.clone();
            Some(Box::new(move |script: &mut UiScript| {
                tails.borrow_mut().push(script.input_events());
            }))
        }
    }

    /// A call an engine turn left (an event its engine step signalled) waits for the host and
    /// applies under the events its verb ran under, none, though a key's body is the entry that
    /// lends it, and before that body runs, so the body reads the state it made.
    #[test]
    fn an_engine_turns_call_applies_under_its_own_events_when_the_host_is_lent() {
        let mut s = UiScript::new().unwrap();
        s.host_out(|s| s.run(r#"TargetUnit("player")"#)).unwrap();
        let mut host = Events::default();
        s.hosted(&mut host, |s| s.run_binding("ClearTarget()"))
            .unwrap();
        assert_eq!(
            host.applied,
            vec![
                (select("player"), InputEvents::NONE),
                (ScriptCall::ClearTarget, KEY)
            ]
        );
        assert_eq!(*host.tails.borrow(), vec![InputEvents::NONE, KEY]);
    }

    /// An engine turn with no host to lend after drops what it left: nothing waits past the entry
    /// that made it.
    #[test]
    fn an_engine_turn_with_no_host_drops_what_it_left() {
        let mut s = UiScript::new().unwrap();
        s.host_out(|s| s.run(r#"TargetUnit("player") ClearTarget()"#))
            .unwrap();
        assert_eq!(s.refuse_waiting(), 2);
        let mut host = Events::default();
        s.hosted(&mut host, |_| ());
        assert!(host.applied.is_empty());
    }

    /// A call queued with the host out, under no input event, applies under none and its tail
    /// runs under none, though a key's body raised its event by the time it is applied.
    #[test]
    fn a_call_and_its_tail_never_run_under_the_draining_entrys_events() {
        let mut s = UiScript::new().unwrap();
        let mut host = Events::default();
        s.hosted(&mut host, |s| {
            let lua = s.lua().clone();
            super::super::host::with_host(&lua, |_| s.run(r#"TargetUnit("player")"#))
                .expect("lent")
                .unwrap();
            s.run_binding("ClearTarget()").unwrap();
        });
        assert_eq!(
            host.applied,
            vec![
                (select("player"), InputEvents::NONE),
                (ScriptCall::ClearTarget, KEY)
            ]
        );
        assert_eq!(*host.tails.borrow(), vec![InputEvents::NONE, KEY]);
    }

    /// A host that applies each call by counting it into the `APPLIED` global, so a handler can
    /// read whether the call it made has applied by its next line. The first call it applies may
    /// signal `SPELLCAST_FAILED` from inside the apply, and leave a tail that copies `SEEN` into
    /// `TAIL_SAW`.
    #[derive(Default)]
    struct Stamping {
        applied: Vec<ScriptCall>,
        signal: bool,
    }

    impl CallHost for Stamping {
        fn apply(&mut self, script: &mut UiScript, call: ScriptCall) -> Option<CallTail> {
            self.applied.push(call);
            script
                .lua()
                .globals()
                .set("APPLIED", self.applied.len())
                .unwrap();
            if !std::mem::take(&mut self.signal) {
                return None;
            }
            script.fire_event("SPELLCAST_FAILED", vec![]);
            Some(Box::new(|script: &mut UiScript| {
                let seen: mlua::Value = script.lua().globals().get("SEEN").unwrap();
                script.lua().globals().set("TAIL_SAW", seen).unwrap();
            }))
        }
    }

    /// A frame whose handler for `event` (and its `OnShow`) clears the target and notes, in `SEEN`,
    /// how many calls had applied by the next line, and appends `<tag>:<applied>` to `ORDER`.
    fn listen(s: &UiScript, name: &str, event: &str, tag: &str) {
        s.run(&format!(
            r#"local f = CreateFrame("Frame", "{name}")
            local function note()
                ClearTarget()
                SEEN = APPLIED
                ORDER = (ORDER or "") .. "{tag}:" .. (APPLIED or 0) .. " "
            end
            f:RegisterEvent("{event}")
            f:SetScript("OnEvent", note)
            f:SetScript("OnShow", note)"#
        ))
        .unwrap();
    }

    fn global<T: mlua::FromLua>(s: &UiScript, name: &str) -> T {
        s.lua().globals().get(name).unwrap()
    }

    /// The reference has no queue between a verb and its effect: an event an engine step signals
    /// runs its handlers then (`SignalEvent`, `0x703e50`, walks the listeners in place), and a
    /// call they make applies before its verb returns. An engine
    /// turn cannot lend the host, so the handlers it fires run when its entry lends it, and the
    /// call a handler makes applies in place: `SEEN` reads it applied.
    #[test]
    fn a_handler_an_engine_turn_fires_runs_with_the_host_lent() {
        let mut s = UiScript::new().unwrap();
        listen(&s, "Turned", "LOOT_OPENED", "e");
        s.host_out(|s| s.fire_event("LOOT_OPENED", vec![]));
        let mut host = Stamping::default();
        s.hosted(&mut host, |_| ());
        assert_eq!(host.applied, vec![ScriptCall::ClearTarget]);
        assert_eq!(
            global::<Option<u32>>(&s, "SEEN"),
            Some(1),
            "applied in place"
        );
    }

    /// A widget handler an engine turn fires (here `OnShow`) waits for the host the same way.
    #[test]
    fn a_widget_handler_an_engine_turn_fires_runs_with_the_host_lent() {
        let mut s = UiScript::new().unwrap();
        listen(&s, "Shown", "BAG_UPDATE", "s");
        let h = s.frame_handle("Shown").expect("the frame");
        let id = s.model_mut().frame_id(h);
        s.host_out(|s| {
            let lua = s.lua().clone();
            super::super::event::fire_widget_handler(&lua, id, "OnShow", vec![]).unwrap();
        });
        let mut host = Stamping::default();
        s.hosted(&mut host, |_| ());
        assert_eq!(
            global::<Option<u32>>(&s, "SEEN"),
            Some(1),
            "applied in place"
        );
    }

    /// A handler a call's apply fires runs as the host comes back, before the call's tail, so the
    /// call it makes applies in place, inside the handler, and the tail sees it ran.
    #[test]
    fn a_handler_an_apply_fires_runs_as_the_host_is_back_before_the_tail() {
        let mut s = UiScript::new().unwrap();
        listen(&s, "Applied", "SPELLCAST_FAILED", "a");
        let mut host = Stamping {
            signal: true,
            ..Default::default()
        };
        s.hosted(&mut host, |s| s.run(r#"TargetUnit("player")"#))
            .unwrap();
        assert_eq!(
            host.applied,
            vec![select("player"), ScriptCall::ClearTarget]
        );
        assert_eq!(
            global::<Option<u32>>(&s, "SEEN"),
            Some(2),
            "applied in place"
        );
        assert_eq!(
            global::<Option<u32>>(&s, "TAIL_SAW"),
            Some(2),
            "before the tail"
        );
    }

    /// What an engine turn left runs in the order it was made: a handler fired before a waiting
    /// call reads the state before it, one fired after it the state after.
    #[test]
    fn a_turns_handlers_and_waiting_calls_run_in_the_order_made() {
        let mut s = UiScript::new().unwrap();
        listen(&s, "First", "LOOT_OPENED", "1");
        listen(&s, "Second", "LOOT_CLOSED", "2");
        s.host_out(|s| {
            s.fire_event("LOOT_OPENED", vec![]);
            s.run(r#"TargetUnit("player")"#).unwrap();
            s.fire_event("LOOT_CLOSED", vec![]);
        });
        let mut host = Stamping::default();
        s.hosted(&mut host, |_| ());
        // The first handler's call applies inside it, then the waiting one, then the second's.
        assert_eq!(
            global::<Option<String>>(&s, "ORDER").as_deref(),
            Some("1:1 2:3 ")
        );
        assert_eq!(
            host.applied,
            vec![
                ScriptCall::ClearTarget,
                select("player"),
                ScriptCall::ClearTarget
            ]
        );
    }

    /// With no host to lend after the turn, its handlers still run, as it ends: the calls they
    /// make are dropped with the turn's own.
    #[test]
    fn a_turn_with_no_host_runs_its_handlers_as_it_ends() {
        let mut s = UiScript::new().unwrap();
        listen(&s, "Alone", "LOOT_OPENED", "e");
        s.host_out(|s| s.fire_event("LOOT_OPENED", vec![]));
        assert_eq!(
            global::<Option<String>>(&s, "ORDER"),
            None,
            "held for the host"
        );
        assert_eq!(s.refuse_waiting(), 1, "the handler's call");
        assert_eq!(
            global::<Option<String>>(&s, "ORDER").as_deref(),
            Some("e:0 ")
        );
    }

    /// A handler held from inside a `PlaySound` suppression scope (`0x458f50`, the UI load's and
    /// the leave-world sweep's) stays silent when it runs after the scope has closed, and one held
    /// outside any scope plays though it runs inside one.
    #[test]
    fn a_held_handler_plays_under_the_suppression_it_was_fired_under() {
        let mut s = UiScript::new().unwrap();
        s.run(
            r#"local f = CreateFrame("Frame")
            f:RegisterEvent("LOOT_OPENED")
            f:SetScript("OnEvent", function() PlaySound("igMainMenuOpen") end)"#,
        )
        .unwrap();
        let mut host = Stamping::default();
        s.push_sound_suppression();
        s.host_out(|s| s.fire_event("LOOT_OPENED", vec![]));
        s.pop_sound_suppression();
        s.hosted(&mut host, |_| ());
        assert_eq!(s.take_sounds(), vec![], "fired inside the scope");
        s.host_out(|s| s.fire_event("LOOT_OPENED", vec![]));
        s.push_sound_suppression();
        s.hosted(&mut host, |_| ());
        s.pop_sound_suppression();
        assert_eq!(
            s.take_sounds(),
            vec![super::super::SoundRequest::KitName("igMainMenuOpen".into())],
            "fired outside it"
        );
    }

    /// A frame that appends, at each `ITEM_LOCK_CHANGED`, whether items 1 and 2 read locked then:
    /// `1` for locked, `0` for not, as `GetContainerItemInfo`'s `locked` tests the bit
    /// (`0x4f9752`).
    fn watch_locks(s: &UiScript) {
        let lua = s.lua().clone();
        let locked = lua
            .create_function(|lua, guid: u64| {
                Ok(lua
                    .app_data_ref::<Model>()
                    .expect("model app_data")
                    .item_locks
                    .contains(&guid))
            })
            .unwrap();
        lua.globals().set("LOCKED", locked).unwrap();
        s.run(
            r#"SEEN = ""
            local f = CreateFrame("Frame")
            f:RegisterEvent("ITEM_LOCK_CHANGED")
            f:SetScript("OnEvent", function()
                SEEN = SEEN .. (LOCKED(1) and 1 or 0) .. (LOCKED(2) and 1 or 0) .. " "
            end)"#,
        )
        .unwrap();
    }

    fn held_item(guid: u64) -> super::super::cursor::CursorItem {
        super::super::cursor::CursorItem {
            guid,
            bag: 0,
            slot: 1,
            item_id: 2589,
            item_class: None,
            texture: None,
            link: None,
            count: None,
            quality: None,
            equip_slots: Vec::new(),
            bar_placeable: false,
            consumable: false,
        }
    }

    /// `UnlockItem` (`0x495420`) clears the bit (`0x495449`) and signals `ITEM_LOCK_CHANGED`
    /// (`0x495455`) before it returns, so a step that unlocks two items in turn, as a refused move
    /// unlocks its two (`0x5e3ac0`, `0x5e3ad3`), shows the first signal's handlers the second item
    /// still locked. Held for the host, they read the locks as each signal left them.
    #[test]
    fn a_held_handler_reads_the_locks_its_signal_left() {
        let mut s = UiScript::new().unwrap();
        watch_locks(&s);
        s.seed_item_locks([1, 2]);
        s.host_out(|s| {
            s.unlock_item(1);
            s.unlock_item(2);
        });
        let mut host = Stamping::default();
        s.hosted(&mut host, |_| ());
        assert_eq!(global::<String>(&s, "SEEN"), "01 00 ");
        assert_eq!(s.item_locks(), Vec::<u64>::new());
    }

    /// A held verb segment carries its lock writes: the sell-slot clear (`0x4cd640`) unlocks its
    /// item (`0x495420` at `0x4cd653`) after a release that unlocked another, and each signal's
    /// handlers read the locks as it left them. With no host to lend, the turn's handlers read
    /// the same.
    #[test]
    fn a_held_segment_shows_its_handlers_the_locks_it_left_each() {
        for lent in [true, false] {
            let mut s = UiScript::new().unwrap();
            watch_locks(&s);
            s.seed_item_locks([1, 2]);
            s.model_mut().auction_sell_item = Some(held_item(2));
            s.host_out(|s| {
                s.unlock_item(1);
                s.clear_auction_sell_item();
            });
            if lent {
                s.hosted(&mut Stamping::default(), |_| ());
            } else {
                s.refuse_waiting();
            }
            assert_eq!(global::<String>(&s, "SEEN"), "01 00 ", "lent: {lent}");
            assert_eq!(s.item_locks(), Vec::<u64>::new());
        }
    }

    /// A VM whose frame notes, at each `ITEM_LOCK_CHANGED` (`L`) and `CURSOR_UPDATE` (`C`), the
    /// guid on the cursor and whether items 1 and 2 read locked; `PICK_UP(guid)` takes an item onto
    /// the cursor (the setter's clear, the set, then its lock) and `PUT_DOWN()` sends the held one
    /// off (`ClearCursor(0, 1)`). Item 2 is on the cursor, locked.
    fn cursor_watch() -> UiScript {
        use super::super::cursor::{self, Clear, CursorPayload};
        let mut s = UiScript::new().unwrap();
        let lua = s.lua().clone();
        let f = |lua: &Lua, body: fn(&Lua, u64)| {
            lua.create_function(move |lua, guid: Option<u64>| {
                body(lua, guid.unwrap_or(0));
                Ok(())
            })
            .unwrap()
        };
        let pick_up = f(&lua, |lua, guid| {
            cursor::set_cursor(lua, true, |_| Some(CursorPayload::Item(held_item(guid))));
            super::super::tick::verb(lua, |model| cursor::lock_item(model, guid));
        });
        let put_down = f(&lua, |lua, _| {
            super::super::tick::verb(lua, |model| cursor::clear(model, Clear::KEEP_LOCK));
        });
        let watched = lua
            .create_function(|lua, ()| {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                let on = match &model.cursor {
                    Some(CursorPayload::Item(item)) => item.guid,
                    _ => 0,
                };
                let locked = |g| u8::from(model.item_locks.contains(&g));
                Ok(format!("{on}:{}{}", locked(1), locked(2)))
            })
            .unwrap();
        lua.globals().set("PICK_UP", pick_up).unwrap();
        lua.globals().set("PUT_DOWN", put_down).unwrap();
        lua.globals().set("WATCHED", watched).unwrap();
        s.run(
            r#"SEEN = ""
            local f = CreateFrame("Frame")
            f:RegisterEvent("ITEM_LOCK_CHANGED")
            f:RegisterEvent("CURSOR_UPDATE")
            f:SetScript("OnEvent", function()
                local tag = event == "ITEM_LOCK_CHANGED" and "L" or "C"
                SEEN = SEEN .. tag .. ":" .. WATCHED() .. " "
            end)"#,
        )
        .unwrap();
        s.seed_item_locks([2]);
        s.set_cursor_for_test(CursorPayload::Item(held_item(2)));
        s
    }

    /// An engine turn that signals, then clears the cursor `how`, with the signal's handler
    /// running `handler`; what the frame noted once the host is back.
    fn clear_behind(handler: &str, how: super::super::cursor::Clear) -> (UiScript, String) {
        let mut s = cursor_watch();
        s.run(&format!(
            r#"local f = CreateFrame("Frame")
            f:RegisterEvent("PLAYER_TARGET_CHANGED")
            f:SetScript("OnEvent", function() {handler} end)"#
        ))
        .unwrap();
        s.host_out(|s| {
            s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
            let lua = s.lua().clone();
            super::super::tick::verb(&lua, |model| super::super::cursor::clear(model, how));
        });
        s.hosted(&mut Stamping::default(), |_| ());
        let seen = global::<String>(&s, "SEEN");
        (s, seen)
    }

    /// `ClearCursor` (`0x495190`) acts once, on what the cursor holds when it runs: behind a held
    /// signal it runs after that signal's handler, as in 1.12.1, so a pick-up the handler made is
    /// what it takes, unlocked with one `ITEM_LOCK_CHANGED` (`0x4951c2`, `0x4951d9`).
    #[test]
    fn a_waiting_clear_takes_and_unlocks_what_a_handler_ahead_of_it_picked_up() {
        let (s, seen) = clear_behind("PICK_UP(1)", super::super::cursor::Clear::BOTH);
        assert_eq!(
            seen, "L:2:00 C:0:00 C:1:00 L:1:10 L:1:00 C:0:00 ",
            "the pick-up's clear, set and lock, then the waiting clear's own"
        );
        assert!(s.model_ref().cursor.is_none());
        assert_eq!(s.item_locks(), Vec::<u64>::new());
    }

    /// An item send's clear (`ClearCursor(0, 1)`) skips the unlock (`0x4951c2` past `0x4951d9`):
    /// waiting behind a handler's pick-up, it takes that item and leaves it locked.
    #[test]
    fn a_waiting_keep_lock_clear_leaves_a_handlers_pick_up_locked() {
        let (s, seen) = clear_behind("PICK_UP(1)", super::super::cursor::Clear::KEEP_LOCK);
        assert_eq!(seen, "L:2:00 C:0:00 C:1:00 L:1:10 C:0:10 ");
        assert!(s.model_ref().cursor.is_none());
        assert_eq!(s.item_locks(), vec![1]);
    }

    /// A handler that put the held item down leaves the waiting clear an empty cursor: it unlocks
    /// nothing and shows nothing, and the sent item keeps its lock for the server's answer.
    #[test]
    fn a_waiting_clear_behind_a_put_down_unlocks_and_shows_nothing() {
        let (s, seen) = clear_behind("PUT_DOWN()", super::super::cursor::Clear::BOTH);
        assert_eq!(seen, "C:0:01 C:0:01 ");
        assert!(s.model_ref().cursor.is_none());
        assert_eq!(s.item_locks(), vec![2]);
    }

    /// Lua the turn runs in place reads the locks as they are then, and its own unlock waits in
    /// order behind the held signal before it: `ClearCursor` (`0x495190`) on a held item unlocks
    /// it (`0x4951d9`) after the turn's first unlock signalled, so that signal's handler, run once
    /// the host is back, still reads the cursor's item locked.
    #[test]
    fn a_chunks_unlock_waits_behind_the_held_signal_before_it() {
        let mut s = UiScript::new().unwrap();
        watch_locks(&s);
        s.seed_item_locks([1, 2]);
        s.set_cursor_for_test(super::super::cursor::CursorPayload::Item(held_item(2)));
        s.host_out(|s| {
            s.unlock_item(1);
            s.run("ClearCursor()").unwrap();
        });
        s.hosted(&mut Stamping::default(), |_| ());
        // The chunk's own handler ran in place, then the held one.
        assert_eq!(global::<String>(&s, "SEEN"), "00 01 ");
        assert_eq!(s.item_locks(), Vec::<u64>::new());
    }
}
