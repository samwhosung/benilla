//! The tutorial system (the reference's `[0x4b5150, 0x4b5a60)`) behind the stock
//! `TutorialFrame.xml`: two bit banks, the fire-once trigger, the acknowledge-and-send setter,
//! the timers, the popup sound and the trigger sites.
//!
//! Both banks (`0xb711b8`, `0xb711e4`) are filled from the same `SMSG_TUTORIAL_FLAGS`; until it
//! lands nothing fires. Bank A is fire-once: [`TutorialState::trigger`] tests and sets it and raises
//! `TUTORIAL_TRIGGER(id + 1)`, never sending. Bank B is acknowledged: [`TutorialState::acknowledge`]
//! tests it, sets both banks, cancels the id's timer and sends `CMSG_TUTORIAL_FLAG`. So A ⊇ B, and
//! doing the thing suppresses its tutorial, account-wide.
//!
//! `TriggerTutorial(id, delayMs)` (`0x4b5390`): with a zero delay the `TutorialPopup` cue plays and
//! the event fires now; otherwise a timer holds it (the 10 s Targeting popup is the one site). The
//! reference bounds-checks nothing; an id past the bank is a no-op here.
//!
//! The reference has fifty-one trigger sites and seven acknowledge sites (the callers of
//! `0x4b5390` and `0x4b54c0`); the ones this app can produce reach here through
//! [`TutorialEvent`], the polled edges below, or a field watcher's
//! [`crate::field_watch::FieldNotice::Tutorial`], fired in the watcher's place
//! ([`TutorialState::fire_now`]).

use std::time::{Duration, Instant};

use bevy::prelude::*;

use benilla_ui::script::{ScriptValue, UiScript};

use crate::client_state::ClientState;
use crate::net::{ClientCommand, EnteredWorldMessage, NetCommands, ObjectStore, SelfPlayer};
use crate::player::Player;
use crate::query_cache::Ask;
use crate::ui_script::UiFeed;

/// The popup cue `0x4b5390` plays on both legs (`0x846b68`).
const TUTORIAL_POPUP_SOUND: &str = "TutorialPopup";

/// The internal (0-based) ids of the sites this app produces, named by their published title.
pub(crate) mod id {
    pub(crate) const QUESTGIVERS: u32 = 0x00;
    pub(crate) const MOVEMENT: u32 = 0x01;
    pub(crate) const CAMERAS: u32 = 0x02;
    pub(crate) const TARGETING: u32 = 0x03;
    pub(crate) const LOOTING: u32 = 0x06;
    pub(crate) const BACKPACK: u32 = 0x07;
    pub(crate) const TRAINERS: u32 = 0x0d;
    pub(crate) const GROUPING: u32 = 0x11;
    pub(crate) const VENDORS: u32 = 0x13;
    pub(crate) const QUEST_LOG: u32 = 0x14;
    pub(crate) const FRIENDS: u32 = 0x15;
    pub(crate) const CHATTING: u32 = 0x16;
    pub(crate) const EQUIPPABLE_ITEMS: u32 = 0x17;
    pub(crate) const DEATH: u32 = 0x18;
    pub(crate) const RESTED: u32 = 0x19;
    pub(crate) const FATIGUE: u32 = 0x1a;
    pub(crate) const SWIMMING: u32 = 0x1b;
    pub(crate) const BREATH: u32 = 0x1c;
    pub(crate) const RESTING: u32 = 0x1d;
    pub(crate) const HEARTHSTONES: u32 = 0x1e;
    pub(crate) const PVP_COMBAT: u32 = 0x1f;
    pub(crate) const TRAVEL: u32 = 0x22;
    pub(crate) const WELCOME: u32 = 0x29;
    pub(crate) const RANGED_WEAPONS: u32 = 0x2b;
    pub(crate) const RAID_GROUPS: u32 = 0x2d;
    pub(crate) const MEETING_STONES: u32 = 0x2e;
    pub(crate) const BATTLEGROUND_QUEUE: u32 = 0x2f;
    pub(crate) const PORT_TO_BATTLEGROUND: u32 = 0x30;
    pub(crate) const KEYRINGS: u32 = 0x31;
}

/// The level-gated arm of the `SMSG_LEVELUP_INFO` handler (`0x5e38c0`): in the handler's own order,
/// each fires when the new level is at least its threshold.
const LEVEL_TRIGGERS: [(u32, u32); 8] = [
    (3, 0x16),  // Chatting
    (4, 0x0d),  // Trainers
    (4, 0x02),  // Cameras
    (5, 0x26),  // Groups
    (7, 0x15),  // Friends
    (7, 0x25),  // Professions
    (8, 0x20),  // Jumping
    (10, 0x0c), // Learning Talents
];

/// The Targeting popup's delay at its one site (`0x514a8d mov edx,0x2710`).
pub(crate) const TARGETING_DELAY_MS: u32 = 10_000;

/// The Movement popup's silence window (`0x482ff7`: elapsed - stamp - 90000 >= 0), taken as 90 s
/// after world enter with no movement input; what the stamp (`0x5143e0`) holds is untraced.
const MOVEMENT_SILENCE: Duration = Duration::from_millis(90_000);

/// The banks' frame: the world-entry bring-up, then the sites' triggers fired. A field watcher's
/// tutorial tests the same fire-once bank after it ([`TutorialState::fire_now`]).
#[derive(bevy::ecs::schedule::SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct TutorialFeed;

/// A site's ask of the tutorial system, written from wherever the reference's site lives.
#[derive(Message, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TutorialEvent {
    /// `TriggerTutorial(id, delayMs)`: the fire-once leg.
    Trigger { id: u32, delay_ms: u32 },
    /// `SetTutorialFlag(id)` (`0x4b54c0`): the acknowledge-and-send leg.
    Acknowledge { id: u32 },
}

impl TutorialEvent {
    pub(crate) const fn trigger(id: u32) -> Self {
        Self::Trigger { id, delay_ms: 0 }
    }
}

/// A send the drains owe the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TutorialSend {
    Flag(u32),
    Clear,
    Reset,
}

/// A bank bit, byte-wise: the client's `id >> 5` dword indexing on little-endian dwords.
fn bank_bit(bank: &[u8], id: u32) -> Option<bool> {
    bank.get((id >> 3) as usize)
        .map(|b| b & (1 << (id & 7)) != 0)
}

fn set_bank_bit(bank: &mut [u8], id: u32) {
    if let Some(b) = bank.get_mut((id >> 3) as usize) {
        *b |= 1 << (id & 7);
    }
}

/// The tutorial state, shared through `&self` ([`Self::lock`]): the call host acknowledges inside
/// a script call (`InviteToParty`'s `SetTutorialFlag` at `0x5e949c`) while systems that lend the
/// host hold it too.
#[derive(Resource, Default)]
pub(crate) struct Tutorials(std::sync::Mutex<TutorialState>);

impl Tutorials {
    /// The state, held for one step; never across Lua, which may acknowledge in a call.
    pub(crate) fn lock(&self) -> std::sync::MutexGuard<'_, TutorialState> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// `SetTutorialFlag(id)` (`0x4b54c0`) inside a script call: the acknowledge, and its
    /// `CMSG_TUTORIAL_FLAG` sent at once, with any send owed before it, so it leaves ahead of the
    /// call's own packet.
    pub(crate) fn acknowledge_now(&self, id: u32, commands: &NetCommands) {
        let mut state = self.lock();
        state.acknowledge(id);
        state.flush(commands);
    }
}

/// The two banks, the timers, and what the frame owes the VM and the wire.
#[derive(Default)]
pub(crate) struct TutorialState {
    /// Bank A (`0xb711b8`…): fire-once. `None` until the packet lands.
    fire_once: Option<Vec<u8>>,
    /// Bank B (`0xb711e4`…): acknowledged.
    acknowledged: Option<Vec<u8>>,
    /// The delayed triggers (`TUTORIALTIMER` nodes): `(id, due)`.
    timers: Vec<(u32, Instant)>,
    /// Published ids (`id + 1`) whose event and cue are owed this frame.
    fired: Vec<u32>,
    sends: Vec<TutorialSend>,
    /// Item pushes for us since the last feed (`0x491a60`'s sites): `(entry, bag, slot)`.
    pushes: Vec<(u32, u8, u32)>,
}

impl TutorialState {
    /// Send what is owed, in order.
    fn flush(&mut self, commands: &NetCommands) {
        for send in std::mem::take(&mut self.sends) {
            let command = match send {
                TutorialSend::Flag(id) => ClientCommand::TutorialFlag { id },
                TutorialSend::Clear => ClientCommand::TutorialClear,
                TutorialSend::Reset => ClientCommand::TutorialReset,
            };
            let _ = commands.0.send(command);
        }
    }

    /// `SMSG_TUTORIAL_FLAGS`: both banks copied from the payload.
    pub(crate) fn apply_flags(&mut self, bytes: &[u8]) {
        self.fire_once = Some(bytes.to_vec());
        self.acknowledged = Some(bytes.to_vec());
    }

    /// The bring-up `0x4b5330`: both banks zeroed if present (the bit count stays).
    fn world_enter(&mut self) {
        for bank in [&mut self.fire_once, &mut self.acknowledged]
            .into_iter()
            .flatten()
        {
            bank.iter_mut().for_each(|b| *b = 0);
        }
    }

    /// The teardown `0x4b5380`: every pending timer flushed.
    fn world_leave(&mut self) {
        self.timers.clear();
    }

    /// `TriggerTutorial(id, delayMs)` (`0x4b5390`).
    pub(crate) fn trigger(&mut self, id: u32, delay_ms: u32, now: Instant) {
        let Some(published) = self.fire_now(id) else {
            return;
        };
        if delay_ms == 0 {
            self.fired.push(published);
        } else {
            self.timers
                .push((id, now + Duration::from_millis(u64::from(delay_ms))));
        }
    }

    /// `0x4b5390`'s fire-once test: bank A's bit for `id` tested and set, and the published id
    /// (`id + 1`) for the caller to fire, at once with no delay ([`fire_published`]). `None` with
    /// no bank yet, the bit already set, or an id past the bank (the reference would overrun).
    pub(crate) fn fire_now(&mut self, id: u32) -> Option<u32> {
        let bank = self.fire_once.as_mut()?;
        (bank_bit(bank, id) == Some(false)).then(|| {
            set_bank_bit(bank, id);
            id + 1
        })
    }

    /// `SetTutorialFlag(id)` (`0x4b54c0`): bank B's bit gates, both banks set, the id's timer
    /// cancelled, the 0-based id sent. The reference dereferences an absent bank B; here it is a
    /// no-op.
    pub(crate) fn acknowledge(&mut self, id: u32) {
        let Some(bank_b) = self.acknowledged.as_mut() else {
            return;
        };
        if bank_bit(bank_b, id) != Some(false) {
            return;
        }
        self.timers.retain(|(t, _)| *t != id);
        set_bank_bit(bank_b, id);
        if let Some(bank_a) = self.fire_once.as_mut() {
            set_bank_bit(bank_a, id);
        }
        self.sends.push(TutorialSend::Flag(id));
    }

    /// `ClearTutorials()`: the timers flushed, every bit of both banks set, the empty clear sent.
    fn clear(&mut self) {
        self.timers.clear();
        for bank in [&mut self.fire_once, &mut self.acknowledged]
            .into_iter()
            .flatten()
        {
            bank.iter_mut().for_each(|b| *b = 0xFF);
        }
        self.sends.push(TutorialSend::Clear);
    }

    /// `ResetTutorials()`: every bit of both banks cleared, the timers kept, the empty reset sent.
    fn reset(&mut self) {
        for bank in [&mut self.fire_once, &mut self.acknowledged]
            .into_iter()
            .flatten()
        {
            bank.iter_mut().for_each(|b| *b = 0);
        }
        self.sends.push(TutorialSend::Reset);
    }

    /// `ProcessTutorialTimers`: every due node fires the same cue and event.
    fn tick(&mut self, now: Instant) {
        let mut due = Vec::new();
        self.timers.retain(|&(id, at)| {
            if at <= now {
                due.push(id + 1);
                false
            } else {
                true
            }
        });
        self.fired.extend(due);
    }

    /// `SMSG_ITEM_PUSH_RESULT` for one of ours: the item-received handler's five sites, resolved
    /// on the next feed with the item's template.
    pub(crate) fn item_received(&mut self, entry: u32, bag: u8, slot: u32) {
        self.pushes.push((entry, bag, slot));
    }

    #[cfg(test)]
    fn bits(&self) -> (Option<&[u8]>, Option<&[u8]>) {
        (self.fire_once.as_deref(), self.acknowledged.as_deref())
    }
}

/// The last-seen level and swimming state, for the edge-triggered `0x4b5390` sites polled here.
/// The ghost, resting, PvP and rested tutorials ride their field watchers
/// ([`crate::field_watch`]), the ghost's arrival leg the cascade ([`run_world_enter_cascade`]).
#[derive(Default)]
struct SelfWatch {
    level: Option<u32>,
    swimming: Option<bool>,
}

/// The hearthstone's item entry (`0x1b24`).
const HEARTHSTONE: u32 = 6948;
const INVTYPE_RANGED: u32 = 15;
const CLASS_HUNTER: u8 = 3;
/// The keyring bag and its slot band (`edi == 0xff`, slot ∈ [0x51, 0x70]).
const KEYRING_BAG: u8 = 0xff;
const KEYRING_SLOTS: std::ops::RangeInclusive<u32> = 0x51..=0x70;

/// Before the script tick: the sites' asks, the descriptor edges, the timers, the cue and the
/// event, and the acknowledged bank's push to whichever VM is live.
fn feed_tutorials(
    mut hosted_vm: crate::script_calls::HostedVm,
    tutorials: Res<Tutorials>,
    mut asks: MessageReader<TutorialEvent>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    items: Res<crate::items::Items>,
    sounds: Res<crate::sound::MessageSounds>,
    mut told: Local<crate::ui_script::VmMemo<Option<Vec<u8>>>>,
) {
    hosted_vm.run(|mut script| {
        let now = Instant::now();
        let mut tutorials = tutorials.lock();
        for ask in asks.read() {
            match *ask {
                TutorialEvent::Trigger { id, delay_ms } => tutorials.trigger(id, delay_ms, now),
                TutorialEvent::Acknowledge { id } => tutorials.acknowledge(id),
            }
        }
        // The engine's sites: Quest Completion, a turn-in test that passed (`0x4df792`).
        if let Some(script) = script.as_mut() {
            for id in script.take_tutorial_triggers() {
                tutorials.trigger(id, 0, now);
            }
        }

        // The item-received sites: Backpack always; Equippable Items for any non-zero inventory
        // type (the reference's 23-bit mask `0x809200` is not decoded); Hearthstones by entry;
        // Ranged Weapons for a ranged type on a non-hunter; Keyrings by the slot band.
        let pushes = std::mem::take(&mut tutorials.pushes);
        let class = self_q.single().ok().and_then(|s| s.0.unit_class());
        for (entry, bag, slot) in pushes {
            tutorials.trigger(id::BACKPACK, 0, now);
            if let Some(info) = items.template(entry, 0, Ask::Read) {
                if info.inventory_type != 0 {
                    tutorials.trigger(id::EQUIPPABLE_ITEMS, 0, now);
                }
                if info.inventory_type == INVTYPE_RANGED && class != Some(CLASS_HUNTER) {
                    tutorials.trigger(id::RANGED_WEAPONS, 0, now);
                }
            }
            if entry == HEARTHSTONE {
                tutorials.trigger(id::HEARTHSTONES, 0, now);
            }
            if bag == KEYRING_BAG && KEYRING_SLOTS.contains(&slot) {
                tutorials.trigger(id::KEYRINGS, 0, now);
            }
        }

        tutorials.tick(now);

        let Some(script) = script else {
            return;
        };
        let fired = std::mem::take(&mut tutorials.fired);
        // The bank is owed to every VM: what it has been told is per VM, so a `/reload`'s fresh VM
        // is told again.
        let told = told.get(script);
        if *told != tutorials.acknowledged {
            told.clone_from(&tutorials.acknowledged);
            script.set_tutorial_bank(tutorials.acknowledged.clone());
        }
        // Free before any handler runs: one may acknowledge inside a call.
        drop(tutorials);
        for published in fired {
            fire_published(script, &sounds, published);
        }
    })
}

/// `0x4b5390`'s firing of a published id: the `TutorialPopup` cue, then `TUTORIAL_TRIGGER`.
pub(crate) fn fire_published(
    script: &mut UiScript,
    sounds: &crate::sound::MessageSounds,
    published: u32,
) {
    sounds.push_cue(TUTORIAL_POPUP_SOUND);
    script.fire_event(
        "TUTORIAL_TRIGGER",
        vec![ScriptValue::Int(i64::from(published))],
    );
}

/// The polled `0x4b5390` edge sites: the `SMSG_LEVELUP_INFO` arm on the level rising, and the
/// mover entering water. Each fires on a change, never on the login descriptor; the bring-up
/// resets the memory.
fn watch_self(
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    player: Res<Player>,
    mut cascade: ResMut<WorldEnterCascade>,
    mut watch: Local<SelfWatch>,
    mut asks: MessageWriter<TutorialEvent>,
) {
    if std::mem::take(&mut cascade.reset_watch) {
        *watch = SelfWatch::default();
    }
    // The first sight of each field arms silently.
    if let Ok(store) = self_q.single() {
        let level = store.0.unit_level();
        if let (Some(prev), Some(new)) = (watch.level, level) {
            if new > prev {
                for (threshold, id) in LEVEL_TRIGGERS {
                    if new >= threshold {
                        asks.write(TutorialEvent::trigger(id));
                    }
                }
            }
        }
        watch.level = level;
    }
    if let Some(prev) = watch.swimming {
        if !prev && player.swimming {
            asks.write(TutorialEvent::trigger(id::SWIMMING));
        }
    }
    watch.swimming = Some(player.swimming);
}

/// After the script tick: the four verbs' writes, then every send.
pub(crate) fn drain_tutorials(
    mut hosted_vm: crate::script_calls::HostedVm,
    tutorials: Res<Tutorials>,
    commands: Res<NetCommands>,
) {
    hosted_vm.run(|script| {
        let mut tutorials = tutorials.lock();
        if let Some(script) = script {
            for id in script.take_tutorial_flag_requests() {
                tutorials.acknowledge(id);
            }
            for _ in 0..script.take_tutorial_clears() {
                tutorials.clear();
            }
            for _ in 0..script.take_tutorial_resets() {
                tutorials.reset();
            }
        }
        tutorials.flush(&commands);
    })
}

/// The world-enter bring-up: the banks zeroed, then the bank captured during the login handshake
/// applied if there was one.
fn on_world_enter(
    mut entered: MessageReader<EnteredWorldMessage>,
    tutorials: Res<Tutorials>,
    mut cascade: ResMut<WorldEnterCascade>,
) {
    let mut tutorials = tutorials.lock();
    for e in entered.read() {
        tutorials.world_enter();
        if let Some(bytes) = &e.tutorial_flags {
            tutorials.apply_flags(bytes);
        }
        cascade.armed = true;
        cascade.entered_at = Some(Instant::now());
        cascade.moved = false;
        cascade.reset_watch = true;
    }
}

/// The enter-world cascade `0x4908c0`'s two unconditional triggers, Welcome (`0x490a48`) then
/// Questgivers (`0x490a51`), once per world session when the local player exists, each arrival's
/// ghost tutorial ahead of them ([`run_world_enter_cascade`]); and the Movement popup's silence
/// window.
#[derive(Resource, Default)]
pub(crate) struct WorldEnterCascade {
    armed: bool,
    entered_at: Option<Instant>,
    /// A movement input happened this session (the Movement auto-acknowledge site).
    pub(crate) moved: bool,
    /// The descriptor watcher's memory is owed a reset (a new world session).
    reset_watch: bool,
}

/// The arrival's tutorial and the cascade's. Our player's create (`0x5dea50`) runs its ghost state
/// (`0x5deb2a` → `0x5de9c0`) before the cascade (`0x5deb60`): arriving with `PLAYER_FLAGS` bit
/// 0x10 triggers Death (`0x5de9eb`-`0x5de9f0`), at login and at every cross-map port, so it comes
/// ahead of Welcome.
fn run_world_enter_cascade(
    mut cascade: ResMut<WorldEnterCascade>,
    tutorials: Res<Tutorials>,
    self_q: Query<(), With<SelfPlayer>>,
    arrivals: Query<&ObjectStore, Added<SelfPlayer>>,
) {
    let mut tutorials = tutorials.lock();
    let now = Instant::now();
    for store in &arrivals {
        if store.0.player_flags() & 0x10 != 0 {
            tutorials.trigger(id::DEATH, 0, now);
        }
    }
    if cascade.armed && !self_q.is_empty() {
        cascade.armed = false;
        tutorials.trigger(id::WELCOME, 0, now);
        tutorials.trigger(id::QUESTGIVERS, 0, now);
    }
    if let Some(at) = cascade.entered_at {
        if !cascade.moved && now.duration_since(at) >= MOVEMENT_SILENCE {
            cascade.entered_at = None;
            tutorials.trigger(id::MOVEMENT, 0, now);
        }
    }
}

/// The world-input sites (`0x514840`), bundled for the controller: a movement input
/// acknowledges Movement and arms the 10 s Targeting popup; a mouse-look acknowledges Cameras.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct InputHooks<'w> {
    /// Optional, so a controller test's partial app runs without the tutorial plugin.
    asks: Option<MessageWriter<'w, TutorialEvent>>,
    cascade: Option<ResMut<'w, WorldEnterCascade>>,
}

impl InputHooks<'_> {
    /// A movement input this frame (`test ecx,0x1030`, taken as the forward and side axes).
    pub(crate) fn moved(&mut self) {
        if let Some(c) = self.cascade.as_mut() {
            c.moved = true;
        }
        if let Some(asks) = self.asks.as_mut() {
            asks.write(TutorialEvent::Acknowledge { id: id::MOVEMENT });
            asks.write(TutorialEvent::Trigger {
                id: id::TARGETING,
                delay_ms: TARGETING_DELAY_MS,
            });
        }
    }

    /// A mouse-look this frame (`[ebp+8] & 3`, the two buttons).
    pub(crate) fn mouselooked(&mut self) {
        if let Some(asks) = self.asks.as_mut() {
            asks.write(TutorialEvent::Acknowledge { id: id::CAMERAS });
        }
    }
}

/// The window edges the reference's handlers fire from: the trainer list (Trainers acknowledged,
/// `0x4d74ae`), the vendor list (Vendors, `0x4fad32`) and the taxi map (Travel, `0x4dbaa2`). Raid
/// Groups fires from the group list itself (`ui_party::net`).
#[derive(Default)]
struct WindowWatch {
    trainer: Option<u64>,
    vendor: Option<u64>,
    taxi: bool,
}

fn watch_windows(
    trainer: Res<crate::ui_trainer::TrainerOpen>,
    merchant: Res<crate::ui_merchant::MerchantOpen>,
    taxi: Res<crate::ui_taxi::TaxiState>,
    mut watch: Local<WindowWatch>,
    mut asks: MessageWriter<TutorialEvent>,
) {
    if trainer.trainer.is_some() && trainer.trainer != watch.trainer {
        asks.write(TutorialEvent::Acknowledge { id: id::TRAINERS });
    }
    watch.trainer = trainer.trainer;
    if merchant.vendor.is_some() && merchant.vendor != watch.vendor {
        asks.write(TutorialEvent::trigger(id::VENDORS));
    }
    watch.vendor = merchant.vendor;
    let taxi_open = taxi.open.is_some();
    if taxi_open && !watch.taxi {
        asks.write(TutorialEvent::trigger(id::TRAVEL));
    }
    watch.taxi = taxi_open;
}

fn on_world_leave(tutorials: Res<Tutorials>, mut cascade: ResMut<WorldEnterCascade>) {
    tutorials.lock().world_leave();
    cascade.armed = false;
    cascade.entered_at = None;
}

pub(crate) struct TutorialPlugin;

impl Plugin for TutorialPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<Tutorials>()
            .init_resource::<WorldEnterCascade>()
            .add_message::<TutorialEvent>()
            .add_systems(
                Update,
                (
                    on_world_enter.before(run_world_enter_cascade),
                    run_world_enter_cascade.before(feed_tutorials),
                    watch_windows.before(feed_tutorials),
                    watch_self
                        .after(on_world_enter)
                        .after(crate::player::ControlRestore)
                        .before(feed_tutorials),
                    // Not before the in-game UI exists: `fired` is taken once, with no `VmMemo`
                    // behind it, so the login's Welcome and Questgivers fired into the glue VM
                    // during the one frame it is still live in the world would be lost. `fired`
                    // simply waits for the first frame with an interface.
                    feed_tutorials
                        .in_set(UiFeed)
                        .in_set(TutorialFeed)
                        .run_if(crate::ui_script::ingame_ui_up),
                    drain_tutorials.in_set(crate::ui_script::UiDrain),
                ),
            )
            .add_systems(OnExit(ClientState::InWorld), on_world_leave);
    }
}

/// The tutorial flags' packet handler.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::Tutorials;
    use crate::net::NetHandlerApp;

    /// Register the handler, from [`super::TutorialPlugin`].
    pub(super) fn register(app: &mut App) {
        app.net_handler(SessionEventKind::TutorialFlags, on_flags);
    }

    fn on_flags(In(ev): In<SessionEvent>, tutorials: Res<Tutorials>) {
        if let SessionEvent::TutorialFlags(bytes) = ev {
            tutorials.lock().apply_flags(&bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn banked() -> TutorialState {
        let mut t = TutorialState::default();
        t.apply_flags(&[0u8; 32]);
        t
    }

    /// No bank: silent. A clear bit: set in A only, the event owed, nothing sent. Twice: once.
    #[test]
    fn the_trigger_fires_once_per_bank_a_and_never_sends() {
        let now = Instant::now();
        let mut t = TutorialState::default();
        t.trigger(id::WELCOME, 0, now);
        assert!(t.fired.is_empty(), "no bank yet: silent");
        let mut t = banked();
        t.trigger(id::WELCOME, 0, now);
        t.trigger(id::WELCOME, 0, now);
        assert_eq!(t.fired, vec![id::WELCOME + 1], "published as id + 1, once");
        let (a, b) = t.bits();
        assert_eq!(bank_bit(a.unwrap(), id::WELCOME), Some(true));
        assert_eq!(
            bank_bit(b.unwrap(), id::WELCOME),
            Some(false),
            "bank B untouched"
        );
        assert!(t.sends.is_empty(), "the trigger sends nothing");
        t.trigger(300, 0, now);
        assert_eq!(t.fired.len(), 1, "past the bank: a no-op");
    }

    /// The delayed leg holds the id until due; an acknowledge cancels it.
    #[test]
    fn a_delayed_trigger_waits_and_an_acknowledge_cancels_it() {
        let now = Instant::now();
        let mut t = banked();
        t.trigger(id::TARGETING, TARGETING_DELAY_MS, now);
        assert!(t.fired.is_empty());
        t.tick(now + Duration::from_millis(9_999));
        assert!(t.fired.is_empty(), "not yet");
        t.tick(now + Duration::from_millis(10_000));
        assert_eq!(t.fired, vec![id::TARGETING + 1]);

        let mut t = banked();
        t.trigger(id::TARGETING, TARGETING_DELAY_MS, now);
        t.acknowledge(id::TARGETING);
        t.tick(now + Duration::from_millis(20_000));
        assert!(t.fired.is_empty(), "the acknowledge cancelled the timer");
        assert_eq!(t.sends, vec![TutorialSend::Flag(id::TARGETING)]);
    }

    /// The acknowledge: bank B gates, both banks set, one send; a second is silent; absent bank
    /// B is a no-op.
    #[test]
    fn the_acknowledge_sets_both_banks_and_sends_once() {
        let mut t = TutorialState::default();
        t.acknowledge(id::CHATTING);
        assert!(t.sends.is_empty(), "no bank: nothing");
        let mut t = banked();
        t.acknowledge(id::CHATTING);
        t.acknowledge(id::CHATTING);
        assert_eq!(t.sends, vec![TutorialSend::Flag(id::CHATTING)]);
        let (a, b) = t.bits();
        assert_eq!(bank_bit(a.unwrap(), id::CHATTING), Some(true));
        assert_eq!(bank_bit(b.unwrap(), id::CHATTING), Some(true));
        // Acknowledged first: the later trigger is silent (A ⊇ B).
        t.trigger(id::CHATTING, 0, Instant::now());
        assert!(t.fired.is_empty());
    }

    /// Clear sets every bit and flushes the timers; Reset clears every bit and keeps them.
    #[test]
    fn clear_and_reset_write_both_banks() {
        let now = Instant::now();
        let mut t = banked();
        t.trigger(id::TARGETING, TARGETING_DELAY_MS, now);
        t.clear();
        assert!(t.timers.is_empty(), "clear flushes the timers");
        let (a, b) = t.bits();
        assert!(a.unwrap().iter().all(|&x| x == 0xFF) && b.unwrap().iter().all(|&x| x == 0xFF));
        t.reset();
        t.trigger(id::TARGETING, TARGETING_DELAY_MS, now);
        assert_eq!(
            t.timers.len(),
            1,
            "a cleared bank lets the trigger through again"
        );
        t.reset();
        assert_eq!(t.timers.len(), 1, "reset does NOT flush the timers");
        let (a, b) = t.bits();
        assert!(a.unwrap().iter().all(|&x| x == 0) && b.unwrap().iter().all(|&x| x == 0));
        assert_eq!(
            t.sends,
            vec![
                TutorialSend::Clear,
                TutorialSend::Reset,
                TutorialSend::Reset
            ]
        );
    }

    /// World enter zeroes present banks and keeps their size; the packet fills both.
    #[test]
    fn world_enter_zeroes_and_the_packet_fills() {
        let mut t = TutorialState::default();
        t.world_enter();
        assert!(t.fire_once.is_none(), "no bank: nothing to zero");
        t.apply_flags(&[0xFFu8; 4]);
        t.world_enter();
        let (a, b) = t.bits();
        assert_eq!(a.unwrap(), &[0u8; 4]);
        assert_eq!(b.unwrap(), &[0u8; 4]);
        assert_eq!(bank_bit(a.unwrap(), 31), Some(false));
        assert_eq!(bank_bit(a.unwrap(), 32), None, "past a 4-byte bank");
    }

    /// A bare app around [`feed_tutorials`], the one system that owes the VM the bank.
    fn feeder() -> App {
        let mut app = App::new();
        app.init_resource::<Tutorials>()
            .init_resource::<crate::items::Items>()
            .init_resource::<crate::sound::MessageSounds>()
            .add_message::<TutorialEvent>()
            .add_systems(Update, feed_tutorials);
        app
    }

    fn enabled(app: &mut App) -> bool {
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .eval::<bool>("return TutorialsEnabled() ~= nil")
            .unwrap()
    }

    /// A `/reload`'s fresh VM is told the bank again, though no world entry refills it. Untold,
    /// `TutorialsEnabled()` answers nil, the Show Tutorials row loads off, and ticking it passes
    /// `UIOptionsFrame.lua:319`'s `~=` guard into `ResetTutorials()`, re-arming every dismissed
    /// popup account-wide.
    #[test]
    fn a_rebuilt_vm_is_told_the_acknowledged_bank_again() {
        let mut app = feeder();
        // One unacknowledged bit is all `TutorialsEnabled()` scans for.
        let mut bank = vec![0xFFu8; 32];
        bank[0] = 0xFE;
        app.world_mut()
            .resource::<Tutorials>()
            .lock()
            .apply_flags(&bank);

        app.insert_non_send_resource(UiScript::new().expect("VM"));
        app.update();
        assert!(
            enabled(&mut app),
            "the VM that was live when the bank landed"
        );

        // `ReloadUI()`: a fresh VM, and nothing touches the app's banks.
        app.insert_non_send_resource(UiScript::new().expect("VM"));
        assert!(
            !enabled(&mut app),
            "a fresh VM starts knowing no bank — this is the state the row used to load in"
        );
        app.update();
        assert!(enabled(&mut app), "…and must be told it again");
    }

    /// The push is per-VM, not per-frame: a steady session hands the bank over once.
    #[test]
    fn an_unchanged_bank_is_not_re_pushed_every_frame() {
        let mut app = feeder();
        app.world_mut()
            .resource::<Tutorials>()
            .lock()
            .apply_flags(&[0u8; 32]);
        app.insert_non_send_resource(UiScript::new().expect("VM"));
        app.update();
        // Cleared behind the feed's back, a bank the VM was told stays cleared.
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .set_tutorial_bank(None);
        app.update();
        assert!(!enabled(&mut app), "unchanged ⇒ nothing pushed");
        // A real change is still pushed, on the same VM.
        app.world_mut()
            .resource::<Tutorials>()
            .lock()
            .acknowledge(id::CHATTING);
        app.update();
        assert!(enabled(&mut app), "a moved bank reaches the live VM");
    }

    /// The flag tutorials are field watchers' (`crate::field_watch`), which a first create skips:
    /// the self descriptor's poll raises none, whatever its flags became since it last looked.
    #[test]
    fn the_self_poll_raises_no_flag_tutorial() {
        use benilla_protocol::messages::ObjectType;
        use benilla_protocol::ObjectFields;
        let mut app = App::new();
        app.init_resource::<Player>()
            .init_resource::<WorldEnterCascade>()
            .add_message::<TutorialEvent>()
            .add_systems(Update, watch_self);
        let store = |pairs: &[(u16, u32)]| {
            ObjectStore(ObjectFields::from_pairs(pairs).into_created(ObjectType::Player))
        };
        let me = app.world_mut().spawn((SelfPlayer, store(&[(34, 10)]))).id();
        app.update();
        app.world_mut().entity_mut(me).insert(store(&[
            (34, 10),
            (190, 0x30),
            (46, 0x1000),
            (194, 1 << 24),
        ]));
        app.update();
        let asked: Vec<TutorialEvent> = app
            .world_mut()
            .resource_mut::<Messages<TutorialEvent>>()
            .drain()
            .collect();
        assert!(asked.is_empty(), "{asked:?}");
    }

    /// The login's own tutorials wait for an interface, on the real plugin's registration:
    /// The engine's ask, a passing turn-in test's Quest Completion (`0x4df792`), goes through the
    /// fire-once bank to `TUTORIAL_TRIGGER(34)`.
    #[test]
    fn a_completable_quest_triggers_the_quest_completion_tutorial() {
        let mut app = App::new();
        app.init_resource::<Tutorials>()
            .init_resource::<crate::items::Items>()
            .init_resource::<crate::sound::MessageSounds>()
            .add_message::<TutorialEvent>()
            .add_systems(Update, feed_tutorials);
        let mut script = UiScript::new().unwrap();
        let mut q = crate::ui_quest_log::fixture::quest(7, "Deliver the Message", 1);
        q.record.objectives[0] = crate::ui_quest_log::fixture::kill(40, 3);
        q.slot.counters[0] = 3;
        crate::ui_quest_log::fixture::seat(&mut script, &[q]);
        script
            .run(
                r#"
                SEEN = {}
                local f = CreateFrame("Frame")
                f:RegisterEvent("TUTORIAL_TRIGGER")
                f:SetScript("OnEvent", function() table.insert(SEEN, arg1) end)
                local _, _, _, _, _, c = GetQuestLogTitle(2)
                assert(c == 1)
            "#,
            )
            .unwrap();
        app.insert_non_send_resource(script);
        app.world_mut()
            .resource::<Tutorials>()
            .lock()
            .apply_flags(&[0u8; 32]);
        app.update();
        let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
        s.resolve();
        assert_eq!(
            s.eval::<Vec<i64>>("return SEEN").unwrap(),
            [34],
            "the engine's 0x21, published as `TUTORIAL_TITLE34`"
        );
    }

    /// `TUTORIAL_TRIGGER` is fired once, so one published to a frameless VM is gone for the
    /// session.
    #[test]
    fn the_world_enter_tutorials_are_not_fired_into_a_ui_less_vm() {
        let mut app = App::new();
        app.add_plugins(TutorialPlugin)
            .init_resource::<crate::items::Items>()
            .init_resource::<crate::sound::MessageSounds>()
            // The other watchers' inputs, so the real plugin can run.
            .init_resource::<crate::ui_trainer::TrainerOpen>()
            .init_resource::<crate::ui_merchant::MerchantOpen>()
            .init_resource::<crate::ui_taxi::TaxiState>()
            .init_resource::<Player>()
            .add_message::<EnteredWorldMessage>();
        let (tx, _rx) = crossbeam_channel::unbounded();
        app.insert_resource(crate::net::NetCommands(tx));

        // The deferral window: in the world on the wire, the entry load still owed, and a live
        // VM with no frames.
        app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
        app.insert_resource(crate::ui_script::PendingEntryUiLoad);
        app.insert_non_send_resource(UiScript::new().expect("a frameless VM"));

        {
            let t = app.world().resource::<Tutorials>();
            let mut t = t.lock();
            t.apply_flags(&[0u8; 32]);
            t.trigger(id::WELCOME, 0, Instant::now());
        }

        app.update();
        assert_eq!(
            app.world().resource::<Tutorials>().lock().fired,
            vec![id::WELCOME + 1],
            "the window must not publish: there is no frame registered for TUTORIAL_TRIGGER, and \
             the take is one-way"
        );

        // The interface comes up; the same frame delivers what was waiting.
        app.world_mut()
            .remove_resource::<crate::ui_script::PendingEntryUiLoad>();
        app.update();
        assert!(
            app.world().resource::<Tutorials>().lock().fired.is_empty(),
            "…and once there is an interface, it is published exactly once"
        );
    }

    /// The active player's arrival runs its ghost state (`0x5dea50` → `0x5deb2a` → `0x5de9c0`)
    /// before the world-enter cascade (`0x5deb60` → `0x4908c0`): arriving with `PLAYER_FLAGS`
    /// bit 0x10 set fires Death (0x18, `0x5de9eb`-`0x5de9f0`) ahead of Welcome and Questgivers,
    /// published as their number plus one; arriving alive fires no Death.
    #[test]
    fn a_ghost_arrival_fires_death_ahead_of_the_cascades_tutorials() {
        const PLAYER_FLAGS: u16 = 190;
        for (flags, published) in [(0x10, vec![0x19, 0x2a, 0x01]), (0, vec![0x2a, 0x01])] {
            let mut app = App::new();
            app.add_plugins(TutorialPlugin)
                .init_resource::<crate::items::Items>()
                .init_resource::<crate::sound::MessageSounds>()
                .init_resource::<crate::ui_trainer::TrainerOpen>()
                .init_resource::<crate::ui_merchant::MerchantOpen>()
                .init_resource::<crate::ui_taxi::TaxiState>()
                .init_resource::<Player>()
                .add_message::<EnteredWorldMessage>();
            let (tx, _rx) = crossbeam_channel::unbounded();
            app.insert_resource(crate::net::NetCommands(tx));
            app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
            let script = UiScript::new().expect("a VM");
            script
                .run(
                    "SEEN = {} local f = CreateFrame('Frame') \
                     f:RegisterEvent('TUTORIAL_TRIGGER') \
                     f:SetScript('OnEvent', function() table.insert(SEEN, arg1) end)",
                )
                .unwrap();
            app.insert_non_send_resource(script);
            app.world_mut().write_message(EnteredWorldMessage {
                tutorial_flags: Some(vec![0u8; 32]),
            });
            app.world_mut().spawn((
                SelfPlayer,
                ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(
                    PLAYER_FLAGS,
                    flags,
                )])),
            ));
            app.update();
            let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
            s.resolve();
            assert_eq!(
                s.eval::<Vec<i64>>("return SEEN").unwrap(),
                published,
                "flags {flags:#x}"
            );
        }
    }
}
