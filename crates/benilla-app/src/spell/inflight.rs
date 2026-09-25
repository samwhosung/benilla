//! The player's own cast state: the pending cast (`SPELLCAST` at `0xceac48` and the inflight id
//! `0xceca88`), the queued on-next-swing strike, the running channel (`0xceac58`), the
//! auto-repeat key (`0xceac30`), and the local abort on a move, jump or Esc (`AbortCast
//! 0x6e4940`).

use std::time::{Duration, Instant};

use bevy::prelude::*;

use benilla_ui::script::UiScript;

use crate::creature_anim::{CastEvent, CastEventKind, Casting, PlaySeq};
use crate::net::{ClientCommand, GuidIndex, NetCommands, SelfGuid};
use crate::ui_action::Spells;
use crate::ui_cast::{CastBarEdge, CastBarFeed};

/// The in-flight window from send until `SMSG_SPELL_START` names the cast time; the server
/// answers every `CMSG_CAST_SPELL`, so a packet always clears it first.
const SEND_PROVISIONAL: Duration = Duration::from_secs(5);

/// The same for an item use, kept short: vmangos refuses some `CMSG_USE_ITEM`s with only
/// `SMSG_INVENTORY_CHANGE_FAILURE` and no cast result (`HandleUseItemOpcode`), so this deadline
/// is what reopens casting.
const ITEM_SEND_PROVISIONAL: Duration = Duration::from_millis(1_500);

/// Pushback slack added to the server's cast time at `SMSG_SPELL_START`, so a cast the server
/// delays (`SMSG_SPELL_DELAYED`) still guards past its stretched end.
const CAST_SLACK: Duration = Duration::from_secs(2);

/// Our outstanding cast, armed at send. `TryCast 0x6e4b60` refuses a second cast while the
/// inflight id `0xceca88` is set: the same spell bails silently (`0x6e4d43`), another errors 0x61
/// (`0x6e4d97`) unless the inflight spell is on-next-swing (`Attributes & 0x404`). On-next-swing
/// spells arm [`QueuedMeleeSpell`] instead, so this guard needs no attribute test.
///
/// Cleared by the cast's resolution (`SMSG_SPELL_GO`, a failing `SMSG_CAST_RESULT`,
/// `SMSG_SPELL_FAILED_OTHER`), keyed on the spell id, or by [`local_self_cancel`]; `deadline`
/// covers a lost resolution.
#[derive(Resource, Default)]
pub(crate) struct PendingCast(Option<PendingCastState>);

struct PendingCastState {
    spell_id: u32,
    deadline: Instant,
    /// This record refuses the next press, lights the in-flight ring and gates the by-name walk:
    /// ordinary casts and item uses. The reference writes `0xceca88` for every committed cast
    /// (`0x6e5026`) and discriminates at the gate instead; a ranged shot is recorded here without
    /// guarding, so the `modalNextSpell` test (`0x6e7408`) still matches it.
    guards: bool,
}

impl PendingCast {
    /// A guarding cast is outstanding, inside its deadline.
    pub(crate) fn in_flight(&self, now: Instant) -> bool {
        self.0
            .as_ref()
            .is_some_and(|p| p.guards && now < p.deadline)
    }

    /// The reference's `0xceca88`: the last committed, unresolved cast of any class, which
    /// `HandleCastResult 0x6e7330` tests at `0x6e7408` before reading `modalNextSpell`.
    pub(crate) fn committed(&self, now: Instant) -> Option<u32> {
        self.0
            .as_ref()
            .filter(|p| now < p.deadline)
            .map(|p| p.spell_id)
    }

    /// The guarding cast's spell id, which `IsCurrentAction`'s checked state keys on.
    pub(crate) fn current(&self, now: Instant) -> Option<u32> {
        self.0
            .as_ref()
            .filter(|p| p.guards && now < p.deadline)
            .map(|p| p.spell_id)
    }

    /// Arm at send, the client's `0xceca88` write; a ranged shot passes `guards = false`.
    pub(crate) fn arm(&mut self, spell_id: u32, now: Instant, guards: bool) {
        self.0 = Some(PendingCastState {
            spell_id,
            deadline: now + SEND_PROVISIONAL,
            guards,
        });
    }

    /// Arm on an item use: the client's item use (`0x5d9258`) goes through `TryCast 0x6e4b60`
    /// via `0x6e5a90`, so it writes the same inflight id and meets the same gate. Only the
    /// deadline differs.
    pub(crate) fn arm_item(&mut self, spell_id: u32, now: Instant) {
        self.0 = Some(PendingCastState {
            spell_id,
            deadline: now + ITEM_SEND_PROVISIONAL,
            guards: true,
        });
    }

    /// Tighten the deadline to the server's real cast time once `SMSG_SPELL_START` names it.
    pub(crate) fn refine(&mut self, cast_time_ms: u32, now: Instant) {
        if let Some(p) = &mut self.0 {
            p.deadline = now + Duration::from_millis(u64::from(cast_time_ms)) + CAST_SLACK;
        }
    }

    /// `SMSG_SPELL_DELAYED`: extend from the later of the deadline and now, so a lapsed deadline
    /// re-arms.
    pub(crate) fn delay(&mut self, delay_ms: u32, now: Instant) {
        if let Some(p) = &mut self.0 {
            p.deadline = p.deadline.max(now) + Duration::from_millis(u64::from(delay_ms));
        }
    }

    /// Clear on a resolution for our spell only: a proc's `SMSG_SPELL_GO` mid-cast must not.
    pub(crate) fn clear_if(&mut self, spell_id: u32) {
        if self.0.as_ref().is_some_and(|p| p.spell_id == spell_id) {
            self.0 = None;
        }
    }
}

/// Our queued on-next-swing spell (Heroic Strike, Cleave: `Attributes & 0x404`). The reference
/// keeps it in the inflight id `0xceca88`, and a new cast nests around it (`PushPopNestedCast
/// 0x6e4ad0`); this second slot beside [`PendingCast`] gives the same observable.
///
/// No deadline: cleared, keyed on the id, by `SMSG_SPELL_GO` when the swing fires or by the
/// failing `SMSG_CAST_RESULT` and `SMSG_SPELL_FAILED_OTHER` vmangos sends when the melee slot
/// dies. The reference clears it on a matching `SMSG_CAST_RESULT` alone (`0x6e7330`; its GO
/// handler never touches `0xceca88`); vmangos sends the OK result just before the GO
/// (`Spell.cpp:3681`, `Spell.cpp:3715`). A re-arm replaces it (the server's one
/// `CURRENT_MELEE_SPELL`). Re-pressing the queued spell bails silently (`0x6e4d43`); it un-queues
/// through the StopAttack chain (`0x5ecac0`, `CancelQueuedCast 0x6e6f30`), which an auto-repeat
/// press also runs (`0x6e5976`), or through Esc, which cancels it as the inflight spell
/// (`SpellStopCasting 0x6e6e80`).
#[derive(Resource, Default)]
pub(crate) struct QueuedMeleeSpell(Option<u32>);

impl QueuedMeleeSpell {
    pub(crate) fn current(&self) -> Option<u32> {
        self.0
    }

    pub(crate) fn arm(&mut self, spell_id: u32) {
        self.0 = Some(spell_id);
    }

    pub(crate) fn clear_if(&mut self, spell_id: u32) {
        if self.0 == Some(spell_id) {
            self.0 = None;
        }
    }
}

/// What the reference's one inflight id `0xceca88` holds, joined from our two slots: `IsCasting
/// 0x6e3d30` and `AbortCast 0x6e4940` treat a cast and a queued strike alike.
///
/// A cast started over a queued strike pushes it into the save `0xcecaa8` (`0x6e5026`), so the
/// cast comes first and the first Esc takes it, the second the strike. Deviation: the reference's
/// second Esc re-asserts the strike from the save it never clears (`0x6e4b2c`), where ours clears,
/// because vmangos drops the melee slot on the first cancel whatever its id
/// (`HandleCancelCastOpcode`) and both clients converge on that echo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Inflight {
    /// An ordinary cast, with a guard, a bar and a `Casting` to reap.
    Cast(u32),
    /// A queued on-next-swing strike: no bar and no [`Casting`], so a cancel is the wire message
    /// and the slot.
    Strike(u32),
}

impl Inflight {
    /// What a cancel names on the wire.
    pub(crate) fn spell_id(self) -> u32 {
        match self {
            Self::Cast(id) | Self::Strike(id) => id,
        }
    }
}

/// The reference's inflight slot from our two. `started` is the self [`Casting`] spell id, which
/// covers a started cast that never armed the send guard.
pub(crate) fn inflight(
    pending: &PendingCast,
    queued_melee: &QueuedMeleeSpell,
    started: Option<u32>,
    now: Instant,
) -> Option<Inflight> {
    pending
        .current(now)
        .or(started)
        .map(Inflight::Cast)
        .or_else(|| queued_melee.current().map(Inflight::Strike))
}

/// Slack past the named channel end, so a lost `MSG_CHANNEL_UPDATE(0)` cannot pin the ring on.
const CHANNEL_SLACK: Duration = Duration::from_secs(2);

/// Our running channel, the client's `0xceac58`: `IsCurrentAction 0x4e53a0` keeps a channeled
/// spell's button checked while it runs. `MSG_CHANNEL_UPDATE(0)` ends it, finished or interrupted.
#[derive(Resource, Default)]
pub(crate) struct ActiveChannel(Option<(u32, Instant)>);

impl ActiveChannel {
    pub(crate) fn current(&self, now: Instant) -> Option<u32> {
        self.0
            .filter(|&(_, until)| now < until)
            .map(|(spell, _)| spell)
    }

    /// `MSG_CHANNEL_START`.
    pub(crate) fn start(&mut self, spell_id: u32, duration_ms: u32, now: Instant) {
        self.0 = Some((
            spell_id,
            now + Duration::from_millis(u64::from(duration_ms)) + CHANNEL_SLACK,
        ));
    }

    /// `MSG_CHANNEL_UPDATE`: 0 ends the channel, anything else re-times it (pushback shortens).
    pub(crate) fn update(&mut self, remaining_ms: u32, now: Instant) {
        if remaining_ms == 0 {
            self.0 = None;
        } else if let Some((_, until)) = &mut self.0 {
            *until = now + Duration::from_millis(u64::from(remaining_ms)) + CHANNEL_SLACK;
        }
    }
}

/// The live auto-repeat spell, the client's `0xceac30`: set at cast send (`0x6e54f0`) for
/// `AttributesEx2 & 0x20`, cleared by `SMSG_CANCEL_AUTO_REPEAT` (`0x6ea080`) and a matching cast
/// failure. `IsAutoRepeatAction` and the button flash read it; the sticky
/// `creature_anim::AutoRepeatArmed` is the idle gate.
#[derive(Resource, Default)]
pub(crate) struct AutoRepeatActive(pub Option<u32>);

/// The per-spell movement gates of the local self-cancel, tested at `0x51511e` and `0x515175`.
/// The move keybinds' dispatcher `0x515090` cancels on mask `0x10f0` (forward, back, strafe,
/// autorun; not turn or pitch) and `Script::Jump 0x513bd0` likewise, both calling `AbortCast`
/// for event 0x152 `SPELLCAST_STOP` and `CMSG_CANCEL_CAST`.
pub(crate) const SPELL_INTERRUPT_MOVEMENT: u32 = 0x1; // SpellRec+0x54 InterruptFlags (SPELL_INTERRUPT_FLAG_MOVEMENT)
const CHANNEL_INTERRUPT_MOVING: u32 = 0x8; // SpellRec+0x5c ChannelInterruptFlags (AURA_INTERRUPT_MOVING_CANCELS)

/// A directional move start or a jump happened this frame (the `0x10f0` mask; there is no
/// autorun yet). Set by `player::control`, consumed by [`local_self_cancel`].
#[derive(Resource, Default)]
pub(crate) struct LocalMoveStart(pub(crate) bool);

/// The client's local cancel on a move, jump or Esc mid-cast (`AbortCast 0x6e4940`):
///
/// - Esc first stops a running auto-repeat (`SpellStopCasting 0x6e6e80`, then `0x6ea080`),
///   and that spends the press; the cast survives to the next one.
/// - The cast: `CMSG_CANCEL_CAST`, the self `Casting` reaped with a `Fail` cast event, and the
///   `Stop` then `Interrupted` bar edges. Movement tests the spell's `InterruptFlags`; Esc does
///   not.
/// - The channel, on movement only: `CMSG_CANCEL_CHANNELLING` (`0x6e9b70`) and nothing local;
///   the bar and [`ActiveChannel`] close on `MSG_CHANNEL_UPDATE(0)` (`0x6e75f0`). Esc cannot
///   stop a channel: `0x6e6e80` never calls `0x6e9b70`, and the inflight id is 0 mid-channel.
///
/// The client's own event is the silent 0x152 `SPELLCAST_STOP`, but vmangos answers the cancel
/// with `SMSG_SPELL_FAILED_OTHER` and a failing `SMSG_CAST_RESULT` 0x23, which the client turns
/// into 0x154 `SPELLCAST_INTERRUPTED` (`0x6e1a00`). Our bar edges key on the self `Casting` this
/// reap removes, so the pair is pushed here. The stock `CastingBarFrame.lua:132-139` then steps
/// the flash once per OnUpdate after the 1 s hold, followed by the fade.
pub(super) fn local_self_cancel(
    script: Option<NonSendMut<UiScript>>,
    mut moved: ResMut<LocalMoveStart>,
    mut pending: ResMut<PendingCast>,
    mut queued_melee: ResMut<QueuedMeleeSpell>,
    channel: Res<ActiveChannel>,
    mut auto_repeat: ResMut<AutoRepeatActive>,
    mut feed: ResMut<CastBarFeed>,
    spells: Option<Res<Spells>>,
    net: Res<NetCommands>,
    self_guid: Res<SelfGuid>,
    index: Res<GuidIndex>,
    casting: Query<&Casting>,
    mut cast_events: MessageWriter<CastEvent>,
    mut play_seq: ResMut<PlaySeq>,
    mut ecs: Commands,
) {
    let esc = match script {
        Some(mut s) => s.take_spell_stop(),
        None => false,
    };
    let moved = std::mem::take(&mut moved.0);
    if !esc && !moved {
        return;
    }
    let now = Instant::now();
    let self_e = self_guid.0.as_ref().and_then(|g| index.0.get(g)).copied();
    // Esc stops one thing, auto-repeat first (`0x6e6e80`).
    let esc = if esc && auto_repeat.0.is_some() {
        if *crate::net::CAST_TRACE {
            info!("cast-trace: LOCAL auto-repeat self-cancel (esc), CMSG_CANCEL_AUTO_REPEAT_SPELL");
        }
        crate::creature_anim::cancel_auto_repeat_local(self_e, &mut auto_repeat, &mut ecs, &net);
        false // the press is spent
    } else {
        esc
    };
    if !esc && !moved {
        return;
    }
    let flags_open = |pick: fn(&benilla_formats::SpellDisplay) -> bool, spell_id: u32| {
        // An uncataloged spell cancels, as the server interrupts any ordinary cast.
        spells
            .as_ref()
            .and_then(|s| s.catalog.get(spell_id))
            .is_none_or(pick)
    };
    // `AbortCast` treats a cast and a queued strike alike; only the local teardown differs.
    let started = self_e.and_then(|e| casting.get(e).ok()).map(|c| c.spell_id);
    if let Some(slot) = inflight(&pending, &queued_melee, started, now) {
        let spell_id = slot.spell_id();
        // Movement tests `InterruptFlags`, which spares a queued strike: Heroic Strike 78,
        // Cleave 845 and Raptor Strike 2973 ship 0 (Fireball `0xf`).
        if esc
            || flags_open(
                |d| d.interrupt_flags & SPELL_INTERRUPT_MOVEMENT != 0,
                spell_id,
            )
        {
            if *crate::net::CAST_TRACE {
                info!(
                    "cast-trace: LOCAL self-cancel — {slot:?} ({}), CMSG_CANCEL_CAST",
                    if esc { "esc" } else { "moved" }
                );
            }
            let _ = net.0.send(ClientCommand::CancelCast { spell_id });
            match slot {
                Inflight::Cast(_) => {
                    pending.clear_if(spell_id);
                    // STOP arms the flash, INTERRUPTED paints red and holds, so the flash bursts
                    // after the hold; an interrupt by an enemy has no STOP and no burst.
                    feed.0.push(CastBarEdge::Stop);
                    feed.0.push(CastBarEdge::Interrupted);
                    // `spell_failed_other`'s self reap, run early so the echo finds it done.
                    if let Some(e) = self_e {
                        if casting.get(e).is_ok_and(|c| c.spell_id == spell_id) {
                            ecs.entity(e).remove::<Casting>();
                        }
                        cast_events.write(CastEvent {
                            entity: e,
                            spell_id,
                            kind: CastEventKind::Fail,
                            seq: play_seq.next(),
                        });
                    }
                }
                Inflight::Strike(_) => {
                    // Clearing the slot darkens the checked ring now. `AbortCast` fires 0x152
                    // here too, inert on a hidden bar; there is no INTERRUPTED.
                    queued_melee.clear_if(spell_id);
                    feed.0.push(CastBarEdge::Stop);
                }
            }
        }
    }
    // The channel: movement only, and the send only (`0x6e9b70`).
    if moved {
        if let Some(spell_id) = channel.current(now) {
            if flags_open(
                |d| d.channel_interrupt_flags & CHANNEL_INTERRUPT_MOVING != 0,
                spell_id,
            ) {
                if *crate::net::CAST_TRACE {
                    info!("cast-trace: LOCAL channel self-cancel — spell {spell_id}, CMSG_CANCEL_CHANNELLING");
                }
                let _ = net.0.send(ClientCommand::CancelChannelling { spell_id });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIREBALL: u32 = 133;

    /// A channel whose bar is suppressed still arms [`ActiveChannel`]: the reference writes
    /// `0xceac58` at cast send (`0x6e4f88`) and the channel-start handler `0x6e7550` never
    /// touches it.
    #[test]
    fn suppressing_the_channel_bar_does_not_clear_the_channel_mirror() {
        let t0 = Instant::now();
        // Blood Siphon 24322, a suppressed row.
        let mut channel = ActiveChannel::default();
        channel.start(24322, 8_000, t0);
        assert_eq!(
            channel.current(t0 + Duration::from_secs(1)),
            Some(24322),
            "the wire edge arms the mirror regardless of what the bar does with it"
        );
    }

    #[test]
    fn a_fresh_guard_is_open() {
        let g = PendingCast::default();
        assert!(
            !g.in_flight(Instant::now()),
            "nothing sent yet — casting allowed"
        );
    }

    #[test]
    fn arming_closes_the_guard_and_a_matching_resolution_opens_it() {
        let t0 = Instant::now();
        let mut g = PendingCast::default();
        g.arm(FIREBALL, t0, true);
        assert!(
            g.in_flight(t0),
            "a cast is in flight — a second one is refused"
        );
        g.clear_if(FIREBALL); // SMSG_SPELL_GO / a failing SMSG_CAST_RESULT for our cast
        assert!(
            !g.in_flight(t0),
            "our cast resolved — casting allowed again"
        );
    }

    #[test]
    fn a_different_spells_resolution_leaves_the_guard_closed() {
        let t0 = Instant::now();
        let mut g = PendingCast::default();
        g.arm(FIREBALL, t0, true);
        g.clear_if(FIREBALL + 1); // a triggered proc's GO for some other spell mid-cast
        assert!(
            g.in_flight(t0),
            "a different spell's resolution must not open the guard on our cast"
        );
    }

    #[test]
    fn the_guard_opens_when_its_safety_deadline_lapses() {
        let t0 = Instant::now();
        let mut g = PendingCast::default();
        g.arm(FIREBALL, t0, true);
        assert!(g.in_flight(t0 + SEND_PROVISIONAL - Duration::from_secs(1)));
        assert!(!g.in_flight(t0 + SEND_PROVISIONAL + Duration::from_secs(1)));
    }

    #[test]
    fn a_pushback_extends_the_guard_so_it_holds_past_the_stretched_cast() {
        let t0 = Instant::now();
        let mut g = PendingCast::default();
        g.arm(FIREBALL, t0, true);
        g.refine(1_000, t0); // deadline t0 + 1 s + 2 s slack
        g.delay(500, t0); // pushed to t0 + 3.5 s
        assert!(
            g.in_flight(t0 + Duration::from_millis(3_200)),
            "the pushback kept the guard shut past the original refined deadline"
        );
    }

    #[test]
    fn spell_start_tightens_the_deadline_to_the_real_cast_time() {
        let t0 = Instant::now();
        let mut g = PendingCast::default();
        g.arm(FIREBALL, t0, true);
        g.refine(1_500, t0); // SMSG_SPELL_START: 1.5 s + 2 s slack
        let at_four = t0 + Duration::from_secs(4);
        assert!(
            !g.in_flight(at_four),
            "refined to 1.5s + slack, the guard has opened by t+4s (it would still be shut at the \
             5s provisional)"
        );
    }

    #[test]
    fn queued_melee_spell_is_wire_cleared_and_single_slot() {
        const HEROIC_STRIKE: u32 = 78;
        const CLEAVE: u32 = 845;
        let mut q = QueuedMeleeSpell::default();
        assert_eq!(q.current(), None);

        q.arm(HEROIC_STRIKE);
        assert_eq!(q.current(), Some(HEROIC_STRIKE), "queued and checked");

        // Rend's GO while Heroic Strike waits.
        q.clear_if(772);
        assert_eq!(q.current(), Some(HEROIC_STRIKE));

        q.arm(CLEAVE);
        assert_eq!(q.current(), Some(CLEAVE));

        q.clear_if(CLEAVE);
        assert_eq!(q.current(), None, "the wire resolution opened the queue");
    }

    /// [`local_self_cancel`] in a small App. With no `UiScript` Esc reads false; the Esc tests
    /// insert a VM and call the real `SpellStopCasting()`.
    mod local_cancel {
        use super::*;
        use crate::creature_anim::PlaySeq;
        use crate::net::{Guid, GuidIndex, NetCommands, SelfGuid, SelfPlayer};
        use crate::ui_cast::feed_cast_bar;
        use benilla_formats::{SpellCatalog, SpellDisplay};
        use bevy::ecs::system::RunSystemOnce;
        use std::collections::HashMap;

        const ARCANE_MISSILES: u32 = 5143;
        const HEROIC_STRIKE: u32 = 78;

        fn harness(
            displays: HashMap<u32, SpellDisplay>,
        ) -> (App, crossbeam_channel::Receiver<crate::net::ClientCommand>) {
            let (tx, rx) = crossbeam_channel::unbounded();
            let mut app = App::new();
            app.add_message::<CastEvent>()
                .init_resource::<CastBarFeed>()
                .init_resource::<PendingCast>()
                .init_resource::<QueuedMeleeSpell>()
                .init_resource::<ActiveChannel>()
                .init_resource::<AutoRepeatActive>()
                .init_resource::<LocalMoveStart>()
                .init_resource::<GuidIndex>()
                .init_resource::<SelfGuid>()
                .init_resource::<PlaySeq>()
                .insert_resource(NetCommands(tx))
                .insert_resource(crate::ui_action::Spells {
                    catalog: SpellCatalog::from_displays(displays),
                    ..crate::ui_action::Spells::empty_for_tests()
                });
            let self_e = app.world_mut().spawn((Guid(10), SelfPlayer)).id();
            app.world_mut()
                .resource_mut::<GuidIndex>()
                .0
                .insert(10, self_e);
            app.world_mut().resource_mut::<SelfGuid>().0 = Some(10);
            (app, rx)
        }

        /// The `Casting` an `SMSG_SPELL_START` echo leaves on the self player.
        fn mark_casting(app: &mut App, spell_id: u32) {
            let self_e = app.world().resource::<GuidIndex>().0[&10];
            app.world_mut().entity_mut(self_e).insert(Casting {
                spell_id,
                until: None,
            });
        }

        fn run(app: &mut App, moved: bool) {
            app.world_mut().resource_mut::<LocalMoveStart>().0 = moved;
            app.world_mut().run_system_once(local_self_cancel).unwrap();
        }

        #[test]
        fn a_move_edge_mid_cast_cancels_locally() {
            let (mut app, rx) = harness(HashMap::from([(
                FIREBALL,
                SpellDisplay {
                    interrupt_flags: 0xf, // the ordinary timed-cast mask (movement bit set)
                    ..Default::default()
                },
            )]));
            app.world_mut()
                .resource_mut::<PendingCast>()
                .arm(FIREBALL, Instant::now(), true);
            mark_casting(&mut app, FIREBALL);

            run(&mut app, true);

            assert!(
                matches!(
                    rx.try_recv(),
                    Ok(crate::net::ClientCommand::CancelCast { spell_id: FIREBALL })
                ),
                "the move edge ships CMSG_CANCEL_CAST for the in-flight cast"
            );
            assert!(
                !app.world()
                    .resource::<PendingCast>()
                    .in_flight(Instant::now()),
                "the in-flight guard opens with the local cancel"
            );
            let feed = &app.world().resource::<CastBarFeed>().0;
            assert_eq!(feed.len(), 2);
            assert!(
                matches!(
                    (&feed[0], &feed[1]),
                    (CastBarEdge::Stop, CastBarEdge::Interrupted)
                ),
                "the bar edges are the ref's own two-step at RTT→0 — STOP (arms the flash \
                 overlay) then the red INTERRUPTED (the echo's repaint: hold, burst, fade); \
                 the keyed reap silences the real echo (0449/0454)"
            );
            let self_e = app.world().resource::<GuidIndex>().0[&10];
            assert!(
                app.world().entity(self_e).get::<Casting>().is_none(),
                "the self `Casting` state is reaped, so the server's echo (FAILED_OTHER + \
                 CAST_RESULT) finds nothing to repaint"
            );
            assert!(
                !app.world().resource::<LocalMoveStart>().0,
                "the edge is consumed"
            );
        }

        /// `InterruptFlags` without the movement bit keeps casting (`0x51511e`).
        #[test]
        fn the_interrupt_flags_gate_spares_a_movement_castable_spell() {
            let (mut app, rx) = harness(HashMap::from([(
                FIREBALL,
                SpellDisplay {
                    interrupt_flags: 0xe, // movement bit clear
                    ..Default::default()
                },
            )]));
            app.world_mut()
                .resource_mut::<PendingCast>()
                .arm(FIREBALL, Instant::now(), true);

            run(&mut app, true);

            assert!(rx.try_recv().is_err(), "no wire cancel for a spared spell");
            assert!(
                app.world()
                    .resource::<PendingCast>()
                    .in_flight(Instant::now()),
                "the guard keeps holding — the cast runs on"
            );
            assert!(app.world().resource::<CastBarFeed>().0.is_empty());
        }

        #[test]
        fn an_uncataloged_spell_fails_open() {
            let (mut app, rx) = harness(HashMap::new());
            app.world_mut()
                .resource_mut::<PendingCast>()
                .arm(FIREBALL, Instant::now(), true);

            run(&mut app, true);

            assert!(
                matches!(
                    rx.try_recv(),
                    Ok(crate::net::ClientCommand::CancelCast { spell_id: FIREBALL })
                ),
                "an unknown spell's cast still cancels on movement"
            );
        }

        /// A started cast with no send guard still cancels through its `Casting`.
        #[test]
        fn a_started_item_cast_cancels_without_the_send_guard() {
            let (mut app, rx) = harness(HashMap::from([(
                FIREBALL,
                SpellDisplay {
                    interrupt_flags: 0xf,
                    ..Default::default()
                },
            )]));
            mark_casting(&mut app, FIREBALL); // PendingCast stays empty

            run(&mut app, true);

            assert!(
                matches!(
                    rx.try_recv(),
                    Ok(crate::net::ClientCommand::CancelCast { spell_id: FIREBALL })
                ),
                "the started cast cancels through the `Casting` half of the union"
            );
            let self_e = app.world().resource::<GuidIndex>().0[&10];
            assert!(app.world().entity(self_e).get::<Casting>().is_none());
        }

        #[test]
        fn no_edge_means_no_cancel() {
            let (mut app, rx) = harness(HashMap::from([(
                FIREBALL,
                SpellDisplay {
                    interrupt_flags: 0xf,
                    ..Default::default()
                },
            )]));
            app.world_mut()
                .resource_mut::<PendingCast>()
                .arm(FIREBALL, Instant::now(), true);

            run(&mut app, false);

            assert!(rx.try_recv().is_err());
            assert!(app
                .world()
                .resource::<PendingCast>()
                .in_flight(Instant::now()));
            assert!(app.world().resource::<CastBarFeed>().0.is_empty());
        }

        /// Mid-channel `SpellStopCasting()` answers nil and nothing is sent.
        #[test]
        fn esc_cannot_stop_a_channel() {
            let (mut app, rx) = harness(HashMap::new());
            app.insert_non_send_resource(UiScript::new().unwrap());
            let now = Instant::now();
            app.world_mut()
                .resource_mut::<ActiveChannel>()
                .start(ARCANE_MISSILES, 5_000, now);

            app.world_mut().run_system_once(feed_cast_bar).unwrap();
            assert_eq!(
                app.world()
                    .non_send_resource::<UiScript>()
                    .eval::<Option<i64>>("return SpellStopCasting()")
                    .unwrap(),
                None,
                "mid-channel the binding answers nil — the vanilla /stopcasting quirk"
            );
            app.world_mut().run_system_once(local_self_cancel).unwrap();
            assert!(
                rx.try_recv().is_err(),
                "no wire cancel — channels break only on the movement path"
            );
            assert_eq!(
                app.world().resource::<ActiveChannel>().current(now),
                Some(ARCANE_MISSILES)
            );
        }

        /// `SpellStopCasting` (`0x6e6e80`) stops the auto-repeat first and spends the press.
        #[test]
        fn esc_cancels_the_auto_repeat_first_and_the_cast_on_the_next_press() {
            const AUTO_SHOT: u32 = 75;
            let (mut app, rx) = harness(HashMap::new());
            app.insert_non_send_resource(UiScript::new().unwrap());
            app.world_mut().resource_mut::<AutoRepeatActive>().0 = Some(AUTO_SHOT);
            app.world_mut()
                .resource_mut::<PendingCast>()
                .arm(FIREBALL, Instant::now(), true);
            mark_casting(&mut app, FIREBALL);

            // Press 1 stops the auto-repeat only.
            app.world_mut().run_system_once(feed_cast_bar).unwrap();
            assert_eq!(
                app.world()
                    .non_send_resource::<UiScript>()
                    .eval::<Option<i64>>("return SpellStopCasting()")
                    .unwrap(),
                Some(1)
            );
            app.world_mut().run_system_once(local_self_cancel).unwrap();
            assert!(
                matches!(
                    rx.try_recv(),
                    Ok(crate::net::ClientCommand::CancelAutoRepeat)
                ),
                "the auto-repeat cancel ships first — the ref's priority branch"
            );
            assert!(rx.try_recv().is_err(), "the cast survives the first press");
            assert!(app.world().resource::<AutoRepeatActive>().0.is_none());
            assert!(
                app.world()
                    .resource::<PendingCast>()
                    .in_flight(Instant::now()),
                "the send guard still holds — one press stopped one thing"
            );
            assert!(
                app.world().resource::<CastBarFeed>().0.is_empty(),
                "an auto-repeat cancel paints no bar edge"
            );

            // Press 2 cancels the cast.
            app.world_mut().run_system_once(feed_cast_bar).unwrap();
            assert_eq!(
                app.world()
                    .non_send_resource::<UiScript>()
                    .eval::<Option<i64>>("return SpellStopCasting()")
                    .unwrap(),
                Some(1)
            );
            app.world_mut().run_system_once(local_self_cancel).unwrap();
            assert!(
                matches!(
                    rx.try_recv(),
                    Ok(crate::net::ClientCommand::CancelCast { spell_id: FIREBALL })
                ),
                "the second press reaches the cast half"
            );
        }

        /// Esc with a strike queued cancels it and returns 1 (`IsCasting 0x6e3d30` is true), so
        /// the Esc ladder stops before `ClearTarget()`.
        #[test]
        fn esc_unqueues_a_strike_and_eats_the_press() {
            let (mut app, rx) = harness(HashMap::from([(
                HEROIC_STRIKE,
                SpellDisplay {
                    attributes: 0x5_0014, // the shipped row: on-next-swing (`& 0x404`)
                    interrupt_flags: 0,   // no movement bit, also shipped
                    ..Default::default()
                },
            )]));
            app.insert_non_send_resource(UiScript::new().unwrap());
            app.world_mut()
                .resource_mut::<QueuedMeleeSpell>()
                .arm(HEROIC_STRIKE);

            app.world_mut().run_system_once(feed_cast_bar).unwrap();
            assert_eq!(
                app.world()
                    .non_send_resource::<UiScript>()
                    .eval::<Option<i64>>("return SpellStopCasting()")
                    .unwrap(),
                Some(1),
                "the queued strike makes IsCasting true, so the binding EATS the press — which \
                 is what spares the target three rungs further down the ladder"
            );
            app.world_mut().run_system_once(local_self_cancel).unwrap();
            assert!(
                matches!(
                    rx.try_recv(),
                    Ok(crate::net::ClientCommand::CancelCast {
                        spell_id: HEROIC_STRIKE
                    })
                ),
                "AbortCast names the queued strike on the wire, exactly as it would a cast"
            );
            assert_eq!(
                app.world().resource::<QueuedMeleeSpell>().current(),
                None,
                "the slot opens locally — the checked ring darkens this frame, not one RTT later"
            );
            assert!(
                matches!(
                    app.world().resource::<CastBarFeed>().0[..],
                    [CastBarEdge::Stop]
                ),
                "the ref's `0x152` STOP and nothing else: no red INTERRUPTED for a spell that \
                 never opened a bar"
            );
        }

        /// Heroic Strike's `InterruptFlags` are 0, so movement leaves it queued.
        #[test]
        fn a_move_edge_leaves_a_queued_strike_queued() {
            let (mut app, rx) = harness(HashMap::from([(
                HEROIC_STRIKE,
                SpellDisplay {
                    interrupt_flags: 0,
                    ..Default::default()
                },
            )]));
            app.world_mut()
                .resource_mut::<QueuedMeleeSpell>()
                .arm(HEROIC_STRIKE);

            run(&mut app, true);

            assert!(rx.try_recv().is_err(), "running at the mob cancels nothing");
            assert_eq!(
                app.world().resource::<QueuedMeleeSpell>().current(),
                Some(HEROIC_STRIKE)
            );
        }

        /// A cast nested over a queued strike (`PushPopNestedCast 0x6e4ad0`) dies on the first
        /// Esc, the strike on the second; this pins the local order only.
        #[test]
        fn esc_takes_the_cast_first_and_the_strike_on_the_next_press() {
            let (mut app, rx) = harness(HashMap::new());
            app.insert_non_send_resource(UiScript::new().unwrap());
            app.world_mut()
                .resource_mut::<QueuedMeleeSpell>()
                .arm(HEROIC_STRIKE);
            app.world_mut()
                .resource_mut::<PendingCast>()
                .arm(FIREBALL, Instant::now(), true);
            mark_casting(&mut app, FIREBALL);

            app.world_mut().run_system_once(feed_cast_bar).unwrap();
            assert_eq!(
                app.world()
                    .non_send_resource::<UiScript>()
                    .eval::<Option<i64>>("return SpellStopCasting()")
                    .unwrap(),
                Some(1)
            );
            app.world_mut().run_system_once(local_self_cancel).unwrap();
            assert!(
                matches!(
                    rx.try_recv(),
                    Ok(crate::net::ClientCommand::CancelCast { spell_id: FIREBALL })
                ),
                "the nested cast is what sits in the inflight slot — it dies first"
            );
            assert!(rx.try_recv().is_err());
            assert_eq!(
                app.world().resource::<QueuedMeleeSpell>().current(),
                Some(HEROIC_STRIKE),
                "the strike is the SAVE underneath, untouched by the first press"
            );

            // Press 2: the pop put the strike back in the slot.
            app.world_mut().run_system_once(feed_cast_bar).unwrap();
            assert_eq!(
                app.world()
                    .non_send_resource::<UiScript>()
                    .eval::<Option<i64>>("return SpellStopCasting()")
                    .unwrap(),
                Some(1),
                "still stoppable — so this press is eaten too, and the target still survives"
            );
            app.world_mut().run_system_once(local_self_cancel).unwrap();
            assert!(matches!(
                rx.try_recv(),
                Ok(crate::net::ClientCommand::CancelCast {
                    spell_id: HEROIC_STRIKE
                })
            ));
            assert_eq!(app.world().resource::<QueuedMeleeSpell>().current(), None);
        }

        /// `0x6e9b70` sends the cancel and fires no event, clearing nothing local.
        #[test]
        fn a_move_edge_mid_channel_sends_the_cancel_and_touches_nothing_local() {
            let (mut app, rx) = harness(HashMap::from([(
                ARCANE_MISSILES,
                SpellDisplay {
                    channel_interrupt_flags: 0x7c0c, // the real 5143 mask (moving bit 0x8 set)
                    ..Default::default()
                },
            )]));
            let now = Instant::now();
            app.world_mut()
                .resource_mut::<ActiveChannel>()
                .start(ARCANE_MISSILES, 5_000, now);

            run(&mut app, true);

            assert!(
                matches!(
                    rx.try_recv(),
                    Ok(crate::net::ClientCommand::CancelChannelling {
                        spell_id: ARCANE_MISSILES
                    })
                ),
                "the move edge ships CMSG_CANCEL_CHANNELLING"
            );
            assert_eq!(
                app.world().resource::<ActiveChannel>().current(now),
                Some(ARCANE_MISSILES),
                "the mirror stays live — the server's UPDATE(0) is what clears it"
            );
            assert!(
                app.world().resource::<CastBarFeed>().0.is_empty(),
                "no local bar edge — the bar closes on the wire's SPELLCAST_CHANNEL_STOP"
            );
        }
    }

    #[test]
    fn active_channel_tracks_the_wire_lifecycle() {
        let t0 = Instant::now();
        let mut ch = ActiveChannel::default();
        assert_eq!(ch.current(t0), None);

        ch.start(10, 5_000, t0); // a 5 s channel
        assert_eq!(ch.current(t0), Some(10));
        assert_eq!(
            ch.current(t0 + Duration::from_secs(4)),
            Some(10),
            "still live near the end"
        );
        assert_eq!(
            ch.current(t0 + Duration::from_secs(8)),
            None,
            "the slack deadline self-clears a lost UPDATE(0)"
        );

        // Pushback: an update naming less time moves the deadline in.
        ch.start(10, 5_000, t0);
        ch.update(1_000, t0);
        assert_eq!(ch.current(t0 + Duration::from_secs(4)), None);

        ch.start(10, 5_000, t0);
        ch.update(0, t0);
        assert_eq!(ch.current(t0), None);
    }
}
