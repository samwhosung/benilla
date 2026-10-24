//! `CGUnit_C::OnDied 0x605860`, which the health watcher runs on every unit's alive-to-dead edge
//! (`0x6046f0` at `0x6047a3`: the new health not above 0, the old above). In its order:
//!
//! - a player's own leg first (`0x5e99b0` at `0x60588d`): a groupmate's death line, here; for
//!   the active player, in its order, the release timer armed ([`crate::death`]'s), the target
//!   cleared, the interaction latch released ([`crate::ui_session`]'s), the loot window closed,
//!   the quest-giver statuses refreshed ([`crate::quest_markers`]'), the cast cancelled and, on a
//!   taxi, the release asked;
//! - the dying unit's attack words zeroed (`0x605896`-`0x6058a8`): its target
//!   ([`crate::creature_anim::Engaged`], `+0xc48`) and its stop in flight (`+0xc54`);
//! - `PresentUnitDeath 0x625190` (`0x6058ae`), the death line, animation and sound, which the chat
//!   log, the animation driver and the creature sounds play off the same edge, and for the active
//!   player its tail, `SignalDeathState` (`0x6251cc` → `0x495850`), `PLAYER_DEAD` and the cursor
//!   cleared ([`crate::death`]'s);
//! - with a spell in flight (`0x6e3d30`) whose record's target (`0xceac60`, read through
//!   `0x6e3d50`) is this unit, `CancelCast(1, 1, 0x65)` (`0x6058e0`);
//! - when the unit is the selection (`0xb4e2d8`), `SetSelection(0, 0)` (`0x605901`) and then
//!   `StopAutoRepeatSpell 0x6ea080` (`0x605906`);
//! - the tail from `0x60590b`: the quest marker, the missiles, the effects, a sound, the auto-move
//!   stop and `0x4931c0` on the unit's guid (`0x605950`).
//!
//! The health watcher runs it inside the update packet that wrote the health (`0x6046f0` at
//! `0x6047a3`). Here the packet's end runs each death's legs in place, so their state and their
//! sends come in packet order, and parks the Lua each leg signals ([`crate::script_calls::defer_vm`]) on the
//! deaths lane, stamped under the health edge's stamp: they fire in stamp order among the other
//! lanes' items, ahead of what the packet raised after the edge, their listeners reading the unit
//! views as of the packet. The death line takes the place between `0x5e99b0`'s list and
//! `PLAYER_DEAD` the legs leave it ([`crate::ui_chat::combat::watch::DeathLineAt`]). A listener reads other state
//! as the frame's packets left it, which #2217 is about.

use std::collections::VecDeque;

use bevy::ecs::system::SystemState;
use bevy::prelude::*;

use benilla_protocol::messages::ObjectType;
use benilla_protocol::{SessionEvent, SessionEventKind};

use super::Selection;
use crate::creature_anim::{AttackSeam, UnitEdit};
use crate::lanes::{Stamp, Until};
use crate::net::{FieldChanged, NetHandlerApp, ObjectStore, SelfPlayer};
use crate::query_cache::Ask;
use crate::ui_action::{FillArg, UiError, UiErrorKeys};

/// What a groupmate's death shows: message `0x4c` (`0x5e9a0e`).
const PLAYER_DIED: &str = "ERR_PLAYER_DIED_S";

/// The Lua the deaths' legs signalled, parked in the VM, each with its stamp and the VM's session
/// and key, for the deaths lane.
#[derive(Resource, Default)]
pub(crate) struct Deaths(VecDeque<(Stamp, u64, u64)>);

/// The VM's end, a reload's or the session's: what its deaths parked goes with it.
pub(crate) fn drop_parked_deaths(world: &mut World) {
    if let Some(mut deaths) = world.get_resource_mut::<Deaths>() {
        deaths.0.clear();
    }
}

/// Registers the deaths on the update packet's end, after the update handler that writes their
/// edges (`crate::net` builds first), and the lane that fires their signals.
pub(super) fn register(app: &mut App) {
    use crate::lanes::LaneApp;
    app.init_resource::<Deaths>()
        .net_handler(SessionEventKind::ObjectUpdateEnd, on_unit_died)
        .add_exclusive_lane("deaths", peek_deaths, fire_deaths, |_| Default::default());
}

/// The packet's deaths, in its edges' order, each run in place.
fn on_unit_died(
    In(ev): In<SessionEvent>,
    world: &mut World,
    edges: &mut SystemState<MessageReader<'static, 'static, FieldChanged>>,
) {
    let SessionEvent::ObjectUpdateEnd = ev else {
        return;
    };
    let edges: Vec<FieldChanged> = edges
        .get_mut(world)
        .read()
        .filter(|e| e.unit_died())
        .copied()
        .collect();
    for e in edges {
        // What the legs stamp nests under the edge, ahead of what the packet raised after it.
        crate::lanes::firing(e.at, || die(world, e));
    }
}

/// The deaths lane's head.
fn peek_deaths(world: &World) -> Option<Stamp> {
    world
        .get_resource::<Deaths>()?
        .0
        .front()
        .map(|(at, ..)| *at)
}

/// Each leg's signals `until` admits, run as the firing of their stamp with the host lent; the
/// lane stops after one whose firing stamped anything, so what it pushed fires next. It reads
/// nothing beside the host and its own lane.
fn fire_deaths(In(until): In<Until>, world: &mut World) {
    loop {
        let Some((at, session, key)) = world
            .resource::<Deaths>()
            .0
            .front()
            .copied()
            .filter(|(at, ..)| until.admits(*at))
        else {
            return;
        };
        world.resource_mut::<Deaths>().0.pop_front();
        let ((), moved) =
            crate::lanes::firing(at, || crate::script_calls::run_parked(world, session, key));
        if moved {
            return;
        }
    }
}

/// Every death's signals queued, fired alone, for a test that drives the deaths without the
/// other lanes.
#[cfg(test)]
pub(crate) fn fire_deaths_lane(world: &mut World) {
    while world
        .resource::<Deaths>()
        .0
        .front()
        .is_some_and(|(at, ..)| Until::all().admits(*at))
    {
        world
            .run_system_cached_with(fire_deaths, Until::all())
            .expect("the deaths lane fires");
    }
}

/// One unit's death, each leg in its place: a deselect's `SetSelection` signals inside it
/// ([`crate::script_calls::ScriptCallHost::engine_select`]), a release of the interaction latch
/// closes its window there ([`crate::ui_session::release_cell`]), and our own death's quest-giver
/// sweep asks there ([`crate::quest_markers::sweep_statuses`]), before the death goes on. What
/// each leg signals is parked behind it ([`Parked`]).
fn die(world: &mut World, e: FieldChanged) {
    let me = world
        .query_filtered::<Entity, With<SelfPlayer>>()
        .iter(world)
        .next();
    let own = e.kind == ObjectType::Player && me == Some(e.entity);
    let mut parked = Parked::new(world);
    // `CGPlayer_C::OnDied 0x5e99b0`, for a player (`0x605882`).
    if own {
        // The active player's leg (`0x5e99d8`-`0x5e99e4` match), in its order.
        parked.step(world, own_death_head, e);
        // `0x4931c0(0xb4e2d0, dl = 0)` (`0x5e9a51`): the latch, with the goodbye.
        parked.leg(world, |world| crate::ui_session::release_latch(world, true));
        parked.step(world, own_death_loot, e);
        // `0x5eb070` (`0x5e9a61`): the quest givers swept.
        parked.leg(world, crate::quest_markers::sweep_statuses);
        parked.step(world, own_death_cast, e);
    } else if e.kind == ObjectType::Player {
        parked.step(world, groupmate_died, e);
    }
    parked.step(world, unit_attack_words, e);
    // `PresentUnitDeath` (`0x6058ae`): the death line's place (`0x62c160`), then for the active
    // player its tail (`0x6251c2`-`0x6251cc`).
    let line = crate::lanes::stamp();
    if let Some(mut at) = world.get_resource_mut::<crate::ui_chat::combat::watch::DeathLineAt>() {
        at.place(e.at, line);
    }
    if own {
        parked.leg(world, crate::death::signal_death_state);
    }
    parked.step(world, unit_died, e);
    // The tail's `0x4931c0` on the unit's guid (`0x605950`, `dl = 1`): no goodbye.
    parked.leg(world, |world| {
        crate::ui_session::release_cell(world, e.guid, false)
    });
    drop(parked);
}

/// A death's deferred stretch on the world's VM: each leg's signals held as it runs and parked
/// behind it on the deaths lane, at a stamp taken as it ends, after what it pushed onto the other
/// lanes. With no VM nothing signals.
struct Parked(Option<(benilla_ui::script::Deferring, usize)>);

impl Parked {
    fn new(world: &World) -> Self {
        Self(crate::script_calls::defer_vm(world))
    }

    /// Run `leg`, then park what it signalled.
    fn leg(&mut self, world: &mut World, leg: impl FnOnce(&mut World)) {
        leg(world);
        let Some((_, mark)) = &self.0 else {
            return;
        };
        if let Some((session, key)) = crate::script_calls::park_since(world, *mark) {
            let at = crate::lanes::stamp();
            world
                .resource_mut::<Deaths>()
                .0
                .push_back((at, session, key));
        }
    }

    /// [`step`] as a leg.
    fn step<M: 'static>(
        &mut self,
        world: &mut World,
        leg: impl IntoSystem<In<FieldChanged>, (), M> + 'static,
        e: FieldChanged,
    ) {
        self.leg(world, |world| step(world, leg, e));
    }
}

/// Run one leg of a death on `e` in place, its unit edits applied as it returns.
fn step<M: 'static>(
    world: &mut World,
    leg: impl IntoSystem<In<FieldChanged>, (), M> + 'static,
    e: FieldChanged,
) {
    if let Err(err) = world.run_system_cached_with(leg, e) {
        // A harness without a leg's state runs the rest.
        debug!("death: a leg did not run: {err}");
    }
    crate::creature_anim::apply_unit_edits(world);
}

/// Our own death's head: the release timer by `PLAYER_FIELD_BYTES` bit 0x8 (`[+0xe68]+0x1028`,
/// `0x5e9a27`), then `SetSelection(0, 0)` (`0x5e9a45`), our attack target still set, as the unit
/// leg zeroes it after.
fn own_death_head(
    In(e): In<FieldChanged>,
    mut vm: crate::script_calls::VmWith<Died<'static, 'static>>,
) {
    let select = {
        let mut d = vm.own();
        let bit = d
            .stores
            .get(e.entity)
            .is_ok_and(|s| s.0.player_release_timer_running());
        let now = d.time.elapsed_secs_f64();
        d.death.arm_release(bit, now);
        d.deselect()
    };
    vm.engine_select(select, Died::pay);
}

/// Our own death past the latch: whatever loot window is open, closed with its release
/// (`0x48f200(1, 0, 0)` at `0x5e9a5c`).
fn own_death_loot(In(_): In<FieldChanged>, mut seam: AttackSeam) {
    seam.close_loot();
}

/// Our own death past the quest-giver sweep: `CancelCast(1, 1, 0x1c)` (`0x5e9a6c`), asking
/// nothing of the slot first; then with `UNIT_FIELD_FLAGS` bit 20 (`0x5e9a7d`) `0x5e0ae0` sends
/// `CMSG_REPOP_REQUEST` at once while the player is dead or a ghost (`0x605f30`), as it is at its
/// death edge.
fn own_death_cast(In(e): In<FieldChanged>, mut d: Died) {
    let seam = &mut d.seam;
    let spells = seam.spells.as_deref();
    seam.slots.core(&seam.net).cancel_reaping(
        true,
        true,
        crate::spell::REASON_ERROR,
        spells,
        Some(e.entity),
        &seam.edits,
    );
    let store = d.stores.get(e.entity).ok();
    if store.is_some_and(|s| {
        s.0.unit_flags() & crate::player::UNIT_FLAG_TAXI_FLIGHT != 0
            && (s.0.unit_is_dead() || s.0.player_is_ghost())
    }) {
        let _ = d.seam.net.0.send(crate::net::ClientCommand::RepopRequest);
    }
}

/// A groupmate's death line: another player than the active one (`0x5e99d8`-`0x5e99e4`) on one of
/// our four party slots (`IsPartyMember 0x4918d0` → `0x4e7f70` at `0x5e99f7`) shows the line under
/// `GetUnitName`'s name, the cache's with a callback or `UNKNOWNOBJECT` (`0x609210` at `0x5e9a08`,
/// `0x496720` at `0x5e9a10`).
fn groupmate_died(In(e): In<FieldChanged>, d: Died) {
    if d.group.party_slots().any(|m| m.guid == e.guid) {
        let store = d.stores.get(e.entity).ok();
        d.errors.push(
            match d.names.resolve_unit(e.guid, store, Ask::Event(&d.seam.net)) {
                Some(name) => UiError::s(PLAYER_DIED, name),
                None => UiError::args(PLAYER_DIED, vec![FillArg::UnknownObject]),
            },
        );
    }
}

/// Any unit's attack words zeroed (`0x605896`-`0x6058a8`): its target and its stop in flight.
fn unit_attack_words(In(e): In<FieldChanged>, mut seam: AttackSeam) {
    seam.edits.push(UnitEdit::EndAttack(e.entity));
    seam.clear_stop_word(e.entity);
}

/// Any unit's legs past `PresentUnitDeath`: a cast whose record names it cancelled (`0x6058e0`),
/// and when it is the selection the deselect (`0x605901`), signalled in place, then the auto-repeat
/// stop (`0x605906`).
fn unit_died(In(e): In<FieldChanged>, mut vm: crate::script_calls::VmWith<Died<'static, 'static>>) {
    let me = vm.own().seam.me.single().ok();
    let selected = {
        let mut d = vm.own();
        let seam = &mut d.seam;
        let spells = seam.spells.as_deref();
        let mut core = seam.slots.core(&seam.net);
        if core.slot().is_some() && core.pending.record_target() == Some(e.guid) {
            core.cancel_reaping(
                true,
                true,
                crate::spell::REASON_TARGETS_DEAD,
                spells,
                me,
                &seam.edits,
            );
        }
        d.selection.guid == Some(e.guid)
    };
    // The dead target becomes the last one; a corpse selected afterwards stays selected, gray.
    if selected {
        let select = vm.own().deselect();
        vm.engine_select(select, Died::pay);
        let mut d = vm.own();
        let seam = &mut d.seam;
        crate::creature_anim::cancel_auto_repeat_local(
            me,
            &mut seam.auto_repeat,
            &seam.edits,
            &mut seam.slots.core(&seam.net),
        );
    }
}

/// What a death's legs read and write, each its own owner's state.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct Died<'w, 's> {
    selection: ResMut<'w, Selection>,
    seam: AttackSeam<'w, 's>,
    /// The release timer our own death arms.
    death: ResMut<'w, crate::death::DeathNet>,
    /// The real clock the release timer counts on, as the death feed reads it.
    time: Res<'w, Time<Real>>,
    group: Res<'w, crate::ui_party::GroupState>,
    names: Res<'w, crate::names::NameCache>,
    stores: Query<'w, 's, &'static ObjectStore>,
    errors: Res<'w, UiErrorKeys>,
}

impl Died<'_, '_> {
    /// `SetSelection(0, 0)`, owing its send when its teardown signals.
    fn deselect(&mut self) -> crate::script_calls::EngineSelect {
        let before = super::click::Standing::of(&self.selection, &self.seam);
        let send = super::click::deselect_owing(&mut self.selection, &mut self.seam);
        before.owing(super::scan::Owed { send, swing: None })
    }

    /// What a deselect owes, paid with nothing to listen first.
    fn pay(mut d: Died, owed: super::SelectDebt) {
        owed.pay(&mut d.selection, &mut d.seam);
    }
}

#[cfg(test)]
mod tests {
    use benilla_protocol::field::{FIELD_UNIT_FLAGS, FIELD_UNIT_HEALTH, FIELD_UNIT_MAXHEALTH};
    use benilla_protocol::messages::ObjectType;
    use benilla_protocol::{ObjectFields, SessionEvent};
    use bevy::ecs::system::RunSystemOnce;
    use bevy::prelude::*;
    use crossbeam_channel::Receiver;

    use crate::creature_anim::{AttackSeam, AttackStopInFlight, Engaged};
    use crate::net::{
        ClientCommand, FieldChanged, Guid, GuidIndex, NetCommands, ObjectStore, SelfGuid,
        SelfPlayer,
    };
    use crate::spell::AutoRepeatActive;
    use crate::target::Selection;
    use crate::ui_cast::{CastBarEdge, CastReport, CastReports};

    const ME: u64 = 0x10;
    const MOB: u64 = 0xF130_0000_0000_0042;
    const OTHER: u64 = 0xF130_0000_0000_0043;
    /// A spell that binds any unit (`Targets 0x2`).
    const FIREBALL: u32 = 133;
    /// Wand Shoot's shape: an auto-repeat spell (`AttributesEx2 0x20`).
    const SHOOT: u32 = 5019;
    /// Mind Vision's second rank: a unit word, 50000 yards.
    const MIND_VISION: u32 = 10909;
    /// A groupmate the client does not hold.
    const UNHELD: u64 = 0x99;

    struct Client {
        app: App,
        me: Entity,
        mob: Entity,
        other: Entity,
        sent: Receiver<ClientCommand>,
    }

    /// The whole client headless, our player and two live mobs streamed, the write channel
    /// captured.
    fn client() -> Client {
        let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
        let (tx, sent) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        let world = app.world_mut();
        world.resource_mut::<SelfGuid>().0 = Some(ME);
        let unit = |kind, extra: &[(u16, u32)]| {
            let mut pairs = vec![(FIELD_UNIT_HEALTH, 100), (FIELD_UNIT_MAXHEALTH, 100)];
            pairs.extend_from_slice(extra);
            ObjectStore(ObjectFields::from_pairs(&pairs).into_created(kind))
        };
        let me = world
            .spawn((
                SelfPlayer,
                Guid(ME),
                Transform::default(),
                unit(ObjectType::Player, &[(2, 0x19), (FIELD_UNIT_FLAGS, 0x8)]),
            ))
            .id();
        let mut mob = |guid| {
            world
                .spawn((
                    Guid(guid),
                    Transform::from_xyz(3.0, 0.0, 0.0),
                    unit(ObjectType::Unit, &[(2, 0x9)]),
                ))
                .id()
        };
        let (mob, other) = (mob(MOB), mob(OTHER));
        let mut index = world.resource_mut::<GuidIndex>();
        index.0.insert(ME, me);
        index.0.insert(MOB, mob);
        index.0.insert(OTHER, other);
        world.insert_resource(crate::ui_action::Spells {
            catalog: benilla_formats::SpellCatalog::from_displays(
                [
                    (
                        FIREBALL,
                        benilla_formats::SpellDisplay {
                            targets: 0x2,
                            ..Default::default()
                        },
                    ),
                    (
                        SHOOT,
                        benilla_formats::SpellDisplay {
                            targets: 0x2,
                            attributes_ex2: 0x20,
                            ..Default::default()
                        },
                    ),
                    (
                        MIND_VISION,
                        benilla_formats::SpellDisplay {
                            implicit_target_a1: 25,
                            range_index: 13,
                            ..Default::default()
                        },
                    ),
                ]
                .into(),
            ),
            ranges: benilla_formats::SpellRangeCatalog::from_rows(
                [(
                    13,
                    benilla_formats::SpellRange {
                        min: 0.0,
                        max: 50_000.0,
                        flags: 0,
                    },
                )]
                .into(),
            ),
            ..crate::ui_action::Spells::empty_for_tests()
        });
        Client {
            app,
            me,
            mob,
            other,
            sent,
        }
    }

    impl Client {
        fn world(&mut self) -> &mut World {
            self.app.world_mut()
        }

        fn select(&mut self, guid: u64, entity: Entity) {
            *self.world().resource_mut::<Selection>() = Selection {
                target: Some(entity),
                guid: Some(guid),
                lit: Some(entity),
                ..Default::default()
            };
        }

        /// A press of `spell_id` at the selection, the ladder's own bind.
        fn cast(&mut self, spell_id: u32) {
            self.world()
                .run_system_once(
                    move |mut ladder: crate::spell::CastLadder,
                          targeting: crate::spell::cast_target::CastTargeting| {
                        let ctx = targeting.context();
                        ladder.send(spell_id, &ctx, crate::spell::CastCommit::Spell);
                    },
                )
                .expect("the ladder runs on the built client");
        }

        /// `unit`'s health falls from 100 to 0 in an update packet, whose end runs the handler.
        fn dies(&mut self, entity: Entity, guid: u64, kind: ObjectType) {
            self.edge(FieldChanged {
                at: crate::lanes::stamp(),
                entity,
                guid,
                kind,
                index: FIELD_UNIT_HEALTH,
                old: 100,
                new: 0,
            });
        }

        /// `edge` written by an update packet's notify pass, and the packet's end.
        fn edge(&mut self, edge: FieldChanged) {
            let world = self.world();
            world.write_message(edge);
            crate::net::handlers::dispatch(world, vec![SessionEvent::ObjectUpdateEnd]);
            super::fire_deaths_lane(world);
        }

        /// A values block on `guid`, one packet, and the frame's lanes.
        fn values(&mut self, guid: u64, pairs: &[(u16, u32)]) {
            let block = SessionEvent::ObjectValues {
                guid,
                fields: ObjectFields::from_pairs(pairs),
            };
            let world = self.world();
            crate::net::handlers::dispatch(world, crate::net::wire([block]));
            super::fire_deaths_lane(world);
            self.lanes();
        }

        /// The frame's lanes, where the packets' events fire.
        fn lanes(&mut self) {
            crate::lanes::fire_in_order(self.world());
        }

        /// The world's VM, its `script` run.
        fn lua(&mut self, script: &str) {
            crate::script_calls::with_hosted_vm(self.world(), |s| s.expect("a VM").run(script))
                .expect("the chunk runs");
        }

        fn sent(&self) -> Vec<ClientCommand> {
            self.sent.try_iter().collect()
        }

        fn reports(&mut self) -> Vec<CastReport> {
            self.world().resource_mut::<CastReports>().take()
        }

        fn in_flight(&mut self) -> Option<u32> {
            self.world()
                .run_system_once(
                    |mut slots: crate::spell::CastSlots, net: Res<NetCommands>| {
                        slots.core(&net).slot().map(|s| s.spell_id())
                    },
                )
                .expect("the slot reads")
        }
    }

    /// Our own death runs `0x5e99b0`'s active-player leg first (`0x605860` at `0x60588d`), in its
    /// order: the target cleared (`0x493540(0, 0)` at `0x5e9a45`), the loot window closed with its
    /// release (`0x48f200(1, 0, 0)` at `0x5e9a5c`), the cast cancelled whatever it is bound to
    /// (`CancelCast(1, 1, 0x1c)` at `0x5e9a6c`), and with `UNIT_FIELD_FLAGS` 0x100000 a release
    /// asked at once (`0x5e9a7d` → `0x5e0ae0`, `CMSG_REPOP_REQUEST`); without it, none.
    #[test]
    fn our_own_death_clears_the_target_closes_the_loot_cancels_the_cast_and_releases() {
        for flagged in [true, false] {
            let mut c = client();
            let (me, mob) = (c.me, c.mob);
            c.select(MOB, mob);
            c.cast(FIREBALL);
            c.world()
                .resource_mut::<crate::ui_loot::LootState>()
                .open(OTHER, 1, 0, Vec::new());
            if flagged {
                c.world()
                    .get_mut::<ObjectStore>(me)
                    .unwrap()
                    .0
                    .merge(ObjectFields::from_pairs(&[(
                        FIELD_UNIT_FLAGS,
                        0x8 | 0x10_0000,
                    )]));
            }
            c.sent();
            c.reports();
            // The health written, whose edge the handler reads.
            c.world()
                .get_mut::<ObjectStore>(me)
                .unwrap()
                .0
                .merge(ObjectFields::from_pairs(&[(FIELD_UNIT_HEALTH, 0)]));

            c.dies(me, ME, ObjectType::Player);
            let mut want = vec![
                "SetSelection { guid: 0 }".to_string(),
                format!("LootRelease {{ guid: {OTHER} }}"),
                format!("CancelCast {{ spell_id: {FIREBALL} }}"),
            ];
            if flagged {
                want.push("RepopRequest".to_string());
            }
            let sent: Vec<String> = c.sent().iter().map(|s| format!("{s:?}")).collect();
            assert_eq!(sent, want, "flagged: {flagged}");
            assert_eq!(c.world().resource::<Selection>().guid, None);
            assert_eq!(
                c.world().resource::<crate::ui_loot::LootState>().source(),
                None
            );
            assert_eq!(c.in_flight(), None, "the cast is popped");
            assert!(
                c.reports()
                    .iter()
                    .any(|r| matches!(r, CastReport::Bar(CastBarEdge::Failed))),
                "and fails"
            );
        }
    }

    /// Our own death runs `0x5e99b0`'s list in its order (`0x5e9a41`-`0x5e9a6c`): the target
    /// cleared (`0x5e9a45`); the interaction latch released with its window's close
    /// (`0x4931c0(0xb4e2d0, 0)` at `0x5e9a51` → `0x493310`); the loot closed with its release
    /// (`0x5e9a5c`); the quest givers asked again (`0x5eb070` at `0x5e9a61`); then the cast
    /// cancelled (`0x5e9a6c`). Its `PLAYER_TARGET_CHANGED` and `MERCHANT_CLOSED` listeners run on
    /// the deaths lane in that order, so their packets follow the death's own.
    #[test]
    fn our_own_death_runs_its_list_in_order() {
        const VENDOR: u64 = 0xF130_0000_0000_0050;
        const GIVER: u64 = 0xF130_0000_0000_0051;
        let mut c = client();
        let (me, mob) = (c.me, c.mob);
        c.select(MOB, mob);
        c.cast(FIREBALL);
        let world = c.world();
        world
            .resource_mut::<crate::ui_loot::LootState>()
            .open(OTHER, 1, 0, Vec::new());
        // A quest giver (`UNIT_NPC_FLAGS` 0x2) whose reaction toward us reads Neutral.
        let giver = world
            .spawn((
                Guid(GIVER),
                crate::net::NetEntity {
                    kind: benilla_protocol::EntityKind::Unit,
                    display_id: None,
                    scale: 1.0,
                },
                Transform::default(),
                ObjectStore(
                    ObjectFields::from_pairs(&[
                        (FIELD_UNIT_HEALTH, 100),
                        (FIELD_UNIT_MAXHEALTH, 100),
                        (benilla_protocol::field::FIELD_UNIT_NPC_FLAGS, 0x2),
                    ])
                    .into_created(ObjectType::Unit),
                ),
            ))
            .id();
        world.resource_mut::<GuidIndex>().0.insert(GIVER, giver);
        crate::ui_merchant::open_merchant(world, VENDOR, Vec::new());
        crate::script_calls::insert_test_vm(world);
        crate::script_calls::with_hosted_vm(world, |s| {
            s.expect("a VM").run(
                r#"local f = CreateFrame("Frame")
                f:RegisterEvent("PLAYER_TARGET_CHANGED")
                f:RegisterEvent("MERCHANT_CLOSED")
                f:SetScript("OnEvent", function()
                    if event == "PLAYER_TARGET_CHANGED" then RandomRoll(1, 1) else RandomRoll(2, 2) end
                end)"#,
            )
        })
        .expect("the listener is in");
        c.sent();
        c.world()
            .get_mut::<ObjectStore>(me)
            .unwrap()
            .0
            .merge(ObjectFields::from_pairs(&[(FIELD_UNIT_HEALTH, 0)]));
        c.dies(me, ME, ObjectType::Player);
        c.lanes();
        let sent: Vec<String> = c
            .sent()
            .iter()
            .filter_map(|s| match s {
                ClientCommand::SetSelection { guid } => Some(format!("SetSelection {guid:#x}")),
                ClientCommand::RandomRoll { min, .. } => Some(format!("RandomRoll {min}")),
                ClientCommand::LootRelease { .. } => Some("LootRelease".to_string()),
                ClientCommand::QuestgiverStatusQuery { npc } if *npc == GIVER => {
                    Some("QuestgiverStatusQuery".to_string())
                }
                ClientCommand::CancelCast { .. } => Some("CancelCast".to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(
            sent,
            [
                "SetSelection 0x0",
                "LootRelease",
                "QuestgiverStatusQuery",
                "CancelCast",
                "RandomRoll 1",
                "RandomRoll 2"
            ]
        );
    }

    /// `PLAYER_FIELD_BYTES`, whose bit 0x8 the release timer reads (`[+0xe68]+0x1028`).
    const BYTES: u16 = 1222;

    /// A client whose session is live and whose VM records `GetReleaseTimeRemaining()` at each
    /// `PLAYER_DEAD`.
    fn dying_client() -> Client {
        let mut c = client();
        let world = c.world();
        world.resource_mut::<crate::net::NetStatus>().connected = true;
        world
            .resource_mut::<Time<bevy::time::Real>>()
            .update_with_duration(std::time::Duration::ZERO);
        crate::script_calls::insert_test_vm(world);
        c.lua(
            r#"DEAD = {} local f = CreateFrame("Frame") f:RegisterEvent("PLAYER_DEAD")
            f:SetScript("OnEvent", function() table.insert(DEAD, GetReleaseTimeRemaining()) end)"#,
        );
        c
    }

    /// What each `PLAYER_DEAD` read of the release timer.
    fn dead_reads(c: &mut Client) -> Vec<f64> {
        crate::script_calls::eval_in_vm(c.world(), "return DEAD")
    }

    /// `0x5e99b0` arms the release timer at our own death by `PLAYER_FIELD_BYTES` bit 0x8 as it
    /// finds it (`[+0xe68]+0x1028`, `0x5e9a27`): now plus 360000 ms when set (`0x5e9a30` →
    /// `0x496250`), else no timer (`0x496280`), which `GetReleaseTimeRemaining` answers with -1
    /// (`0x4962a0`). `PLAYER_DEAD` reads it so in the death's packet, and the bit moving after the
    /// death moves neither.
    #[test]
    fn our_own_death_arms_the_release_timer_by_the_bit_it_finds() {
        for armed in [true, false] {
            let mut c = dying_client();
            let bit = |set: bool| u32::from(set) * 0x8;
            c.values(ME, &[(FIELD_UNIT_HEALTH, 0), (BYTES, bit(armed))]);
            c.values(ME, &[(BYTES, bit(!armed))]);
            crate::death::run_feed_death_once(c.world());
            let left: f64 =
                crate::script_calls::eval_in_vm(c.world(), "return GetReleaseTimeRemaining()");
            let at_death = dead_reads(&mut c);
            assert_eq!(at_death.len(), 1, "armed {armed}: {at_death:?}");
            for read in [left, at_death[0]] {
                if armed {
                    assert!(read > 359.0, "armed: {read}");
                } else {
                    assert_eq!(read, -1.0, "no timer");
                }
            }
        }
    }

    /// Our own death's `PresentUnitDeath` (`0x6058ae`) ends in `SignalDeathState` for the active
    /// player (`0x5fa6d0` at `0x6251c2`, `0x6251cc` → `0x495850`): `PLAYER_DEAD` inside the death's
    /// packet, after `0x5e99b0` armed the timer, so its handlers read the whole window, and the
    /// frame's feed signals it no second time. A second death past the first window reads a fresh
    /// one.
    #[test]
    fn our_own_death_signals_player_dead_in_its_packet_over_the_timer_it_armed() {
        let mut c = dying_client();
        c.values(ME, &[(FIELD_UNIT_HEALTH, 0), (BYTES, 0x8)]);
        let first = dead_reads(&mut c);
        assert_eq!(first.len(), 1, "PLAYER_DEAD in the packet: {first:?}");
        assert!(first[0] > 359.0, "the whole window: {first:?}");
        crate::death::run_feed_death_once(c.world());
        assert_eq!(
            dead_reads(&mut c).len(),
            1,
            "the feed signals it no second time"
        );

        c.values(ME, &[(FIELD_UNIT_HEALTH, 100)]);
        crate::death::run_feed_death_once(c.world());
        c.world()
            .resource_mut::<Time<bevy::time::Real>>()
            .update_with_duration(std::time::Duration::from_secs(400));
        c.values(ME, &[(FIELD_UNIT_HEALTH, 0)]);
        let second = dead_reads(&mut c);
        assert_eq!(second.len(), 2, "{second:?}");
        assert!(second[1] > 359.0, "a fresh window: {second:?}");
        crate::death::run_feed_death_once(c.world());
        assert_eq!(dead_reads(&mut c).len(), 2);
    }

    /// `SignalDeathState` clears the cursor after `PLAYER_DEAD` (`ClearCursor(1, 1)`, `0x495889`
    /// → `0x495190`): held coins go back, and `CURSOR_UPDATE` reads the cursor empty.
    #[test]
    fn our_own_death_clears_the_cursor_after_player_dead() {
        let mut c = dying_client();
        crate::script_calls::with_hosted_vm(c.world(), |s| s.expect("a VM").set_money(500));
        c.lua(
            r#"SEEN = {} local f = CreateFrame("Frame")
            f:RegisterEvent("PLAYER_DEAD") f:RegisterEvent("CURSOR_UPDATE")
            f:SetScript("OnEvent", function() table.insert(SEEN, event .. " " .. GetCursorMoney()) end)
            PickupPlayerMoney(300) SEEN = {}"#,
        );
        c.values(ME, &[(FIELD_UNIT_HEALTH, 0)]);
        let seen: Vec<String> = crate::script_calls::eval_in_vm(c.world(), "return SEEN");
        assert_eq!(seen, ["PLAYER_DEAD 300", "CURSOR_UPDATE 0"]);
    }

    /// The death's sends keep their packet's place among the frame's: the selection's death
    /// deselects (`CMSG_SET_SELECTION` 0, `0x605901`) ahead of the packet after it, whose duel
    /// echo answers at once (`CMSG_DUEL_ACCEPTED`).
    #[test]
    fn a_deaths_sends_go_ahead_of_the_next_packets() {
        let mut c = client();
        let mob = c.mob;
        c.select(MOB, mob);
        let (tx, rx) = crossbeam_channel::unbounded();
        c.world()
            .insert_resource(crate::net::NetEvents::for_test(rx));
        for ev in crate::net::wire([
            SessionEvent::ObjectValues {
                guid: MOB,
                fields: ObjectFields::from_pairs(&[(FIELD_UNIT_HEALTH, 0)]),
            },
            SessionEvent::DuelRequested {
                arbiter: 0x55,
                challenger: ME,
            },
        ]) {
            tx.send(ev).unwrap();
        }
        c.sent();
        for _ in 0..2 {
            let world = c.world();
            crate::net::apply_net_updates(world);
            crate::lanes::fire_in_order(world);
        }
        let order: Vec<&str> = c
            .sent()
            .iter()
            .filter_map(|s| match s {
                ClientCommand::SetSelection { guid: 0 } => Some("SetSelection 0"),
                ClientCommand::DuelAccepted { .. } => Some("DuelAccepted"),
                _ => None,
            })
            .collect();
        assert_eq!(order, ["SetSelection 0", "DuelAccepted"]);
    }

    /// A death's signals parked on a VM that has since been replaced (a reload, the session's
    /// end, the entry load's) run nothing on the new one, whose own parked keys count from 0
    /// again.
    #[test]
    fn a_deaths_signals_parked_on_a_replaced_vm_run_nothing_on_the_next() {
        let mut c = client();
        let mob = c.mob;
        c.select(MOB, mob);
        crate::script_calls::insert_test_vm(c.world());
        c.lua(
            r#"HEARD = 0 local f = CreateFrame("Frame") f:RegisterEvent("PLAYER_TARGET_CHANGED")
            f:SetScript("OnEvent", function() HEARD = HEARD + 1 end)"#,
        );
        // The death's legs run, their signals parked on this VM and left on the lane.
        let world = c.world();
        world.write_message(FieldChanged {
            at: crate::lanes::stamp(),
            entity: mob,
            guid: MOB,
            kind: ObjectType::Unit,
            index: FIELD_UNIT_HEALTH,
            old: 100,
            new: 0,
        });
        crate::net::handlers::dispatch(world, vec![SessionEvent::ObjectUpdateEnd]);
        assert_eq!(world.resource::<Selection>().guid, None, "the legs ran");
        crate::script_calls::insert_test_vm(world);
        crate::script_calls::with_hosted_vm(world, |s| {
            let s = s.expect("a VM");
            s.run(
                r#"HEARD = 0 local f = CreateFrame("Frame") f:RegisterEvent("LOOT_OPENED")
                f:SetScript("OnEvent", function() HEARD = HEARD + 1 end)"#,
            )
            .expect("the listener is in");
            // The new VM parks its own first stretch, under the key the old one's took.
            let mark = s.waiting_mark();
            let deferring = s.defer();
            s.fire_event("LOOT_OPENED", vec![]);
            assert!(s.park_since(mark).is_some());
            drop(deferring);
        });
        super::fire_deaths_lane(c.world());
        let heard: i64 = crate::script_calls::eval_in_vm(c.world(), "return HEARD");
        assert_eq!(
            heard, 0,
            "the stale signals ran the new VM's parked stretch"
        );
    }

    /// The death's legs reserve the death line's place in the net drain, and the line's watcher
    /// takes it in the same frame: the watcher runs after the drain in the real schedule.
    #[test]
    fn the_death_line_watcher_runs_after_the_net_drain() {
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        assert!(crate::test_support::runs_before(
            &mut app,
            crate::net::apply_net_updates,
            crate::ui_chat::combat::watch::death_lines,
        ));
    }

    /// The guid the interaction latch holds.
    fn latched(c: &mut Client) -> Option<u64> {
        c.world()
            .resource::<crate::ui_session::InteractLatch>()
            .guid()
    }

    /// The latched units' lines written since the last look.
    fn vocals(c: &mut Client) -> Vec<crate::ui_session::LatchVocal> {
        c.world()
            .resource_mut::<Messages<crate::ui_session::LatchVocal>>()
            .drain()
            .collect()
    }

    /// A VM that records the windows' closes, in the world.
    fn spy_closes(c: &mut Client) {
        let vm = crate::ui_script::sweep_test::spied_vm(&["MERCHANT_CLOSED", "TRADE_CLOSED"]);
        c.world().insert_non_send_resource(vm);
    }

    /// Our own death releases the latch with the goodbye (`0x4931c0(0xb4e2d0, 0)` at `0x5e9a51`;
    /// the player who is dead hears none, `0x605f30`), closing the window. Another player's death,
    /// a fall of our health that leaves some and a rise out of none release nothing.
    #[test]
    fn our_own_death_releases_the_latch_with_the_goodbye() {
        const STRANGER: u64 = 0x31;
        let mut c = client();
        let stranger = c
            .world()
            .spawn((
                Guid(STRANGER),
                ObjectStore(
                    ObjectFields::from_pairs(&[
                        (FIELD_UNIT_HEALTH, 100),
                        (FIELD_UNIT_MAXHEALTH, 100),
                    ])
                    .into_created(ObjectType::Player),
                ),
            ))
            .id();
        c.world()
            .resource_mut::<GuidIndex>()
            .0
            .insert(STRANGER, stranger);
        spy_closes(&mut c);
        crate::ui_merchant::open_merchant(c.world(), MOB, Vec::new());
        vocals(&mut c);
        c.values(STRANGER, &[(FIELD_UNIT_HEALTH, 0)]);
        c.values(ME, &[(FIELD_UNIT_HEALTH, 20)]);
        assert_eq!(
            latched(&mut c),
            Some(MOB),
            "another player's death, a fall to 20"
        );
        assert!(crate::ui_script::sweep_test::seen_in(&c.app).is_empty());
        c.values(ME, &[(FIELD_UNIT_HEALTH, 0)]);
        assert_eq!(latched(&mut c), None);
        assert_eq!(
            crate::ui_script::sweep_test::seen_in(&c.app),
            ["MERCHANT_CLOSED"]
        );
        assert_eq!(
            vocals(&mut c),
            [crate::ui_session::LatchVocal::Goodbye(MOB)]
        );
        crate::ui_merchant::open_merchant(c.world(), MOB, Vec::new());
        c.values(ME, &[(FIELD_UNIT_HEALTH, 50)]);
        assert_eq!(latched(&mut c), Some(MOB), "a rise out of none");
    }

    /// The latched unit dying closes its window with no goodbye: the tail's `0x4931c0` on the
    /// unit's guid (`dl = 1`, `0x605950`). Another unit's death leaves it, and a latched player's
    /// death, a trade partner's, closes it as any unit's does.
    #[test]
    fn the_latched_units_death_closes_its_window_without_a_goodbye() {
        let mut c = client();
        spy_closes(&mut c);
        crate::ui_merchant::open_merchant(c.world(), MOB, Vec::new());
        vocals(&mut c);
        c.values(OTHER, &[(FIELD_UNIT_HEALTH, 0)]);
        assert_eq!(latched(&mut c), Some(MOB), "another unit's death");
        c.values(MOB, &[(FIELD_UNIT_HEALTH, 0)]);
        assert_eq!(latched(&mut c), None);
        assert_eq!(
            crate::ui_script::sweep_test::seen_in(&c.app),
            ["MERCHANT_CLOSED"]
        );
        assert_eq!(vocals(&mut c), [], "dl = 1: the unit says no goodbye");

        const PARTNER: u64 = 0x77;
        let partner = c
            .world()
            .spawn((
                Guid(PARTNER),
                ObjectStore(
                    ObjectFields::from_pairs(&[
                        (FIELD_UNIT_HEALTH, 100),
                        (FIELD_UNIT_MAXHEALTH, 100),
                    ])
                    .into_created(ObjectType::Player),
                ),
            ))
            .id();
        c.world()
            .resource_mut::<GuidIndex>()
            .0
            .insert(PARTNER, partner);
        let world = c.world();
        world
            .resource_mut::<crate::ui_trade::TradeSession>()
            .begin(PARTNER);
        crate::ui_trade::open_window(world);
        assert_eq!(latched(&mut c), Some(PARTNER));
        vocals(&mut c);
        c.values(PARTNER, &[(FIELD_UNIT_HEALTH, 0)]);
        assert_eq!(latched(&mut c), None, "the trade partner's death");
        assert_eq!(
            crate::ui_script::sweep_test::seen_in(&c.app),
            ["MERCHANT_CLOSED", "TRADE_CLOSED"]
        );
        assert_eq!(vocals(&mut c), [], "the partner says no goodbye");
    }

    /// The health watcher compares signed (`0x604774 jg`, `0x604778 jle`): a rise out of a negative
    /// health to 0 is no death, and a fall to a negative one is.
    #[test]
    fn the_death_edge_reads_health_signed() {
        let mut c = client();
        let me = c.me;
        crate::ui_merchant::open_merchant(c.world(), MOB, Vec::new());
        let edge = |old: u32, new: u32| FieldChanged {
            at: crate::lanes::stamp(),
            entity: me,
            guid: ME,
            kind: ObjectType::Player,
            index: FIELD_UNIT_HEALTH,
            old,
            new,
        };
        c.edge(edge(0x8000_0000, 0));
        assert_eq!(latched(&mut c), Some(MOB), "the old health was not above 0");
        c.edge(edge(20, u32::MAX));
        assert_eq!(latched(&mut c), None, "a fall to -1");
    }

    /// `0x6058b3`-`0x6058e0`: a spell in flight (`0x6e3d30`) whose record target (`0x6e3d50`,
    /// `0xceac60`) is the dying unit is cancelled with `0x6e4940(1, 1, 0x65)`: `CMSG_CANCEL_CAST`,
    /// the pop, and `SPELLCAST_FAILED` for the player's record. Another unit's death leaves it,
    /// and the record target is the bound unit, not the selection.
    #[test]
    fn a_unit_dying_cancels_the_cast_whose_record_names_it() {
        let mut c = client();
        let (mob, other) = (c.mob, c.other);
        c.select(MOB, mob);
        c.cast(FIREBALL);
        assert!(
            c.sent().iter().any(|s| matches!(
                s,
                ClientCommand::CastSpell {
                    spell_id: FIREBALL,
                    target: Some(MOB)
                }
            )),
            "the cast goes out at the mob"
        );
        assert_eq!(c.in_flight(), Some(FIREBALL));
        // The selection moves on; the record keeps the unit the cast bound.
        c.select(OTHER, other);
        c.reports();

        c.dies(other, OTHER, ObjectType::Unit);
        assert!(
            !c.sent()
                .iter()
                .any(|s| matches!(s, ClientCommand::CancelCast { .. })),
            "another unit's death"
        );
        assert_eq!(c.in_flight(), Some(FIREBALL));
        c.reports();

        c.dies(mob, MOB, ObjectType::Unit);
        assert!(
            matches!(
                c.sent()[..],
                [ClientCommand::CancelCast { spell_id: FIREBALL }]
            ),
            "the record's unit died"
        );
        assert_eq!(c.in_flight(), None, "popped");
        assert!(
            c.reports()
                .iter()
                .any(|r| matches!(r, CastReport::Bar(CastBarEdge::Failed))),
            "SPELLCAST_FAILED for SPELL_FAILED_TARGETS_DEAD"
        );
    }

    /// The far cast at a unit the client does not hold parks its guid in the record's target
    /// (`0x6e5454`), which the unit's death reads once it is held (`0x6058c7`).
    #[test]
    fn a_far_casts_parked_target_dying_cancels_it() {
        let mut c = client();
        *c.world().resource_mut::<Selection>() = Selection {
            guid: Some(UNHELD),
            ..Default::default()
        };
        c.cast(MIND_VISION);
        assert!(
            c.sent().iter().any(|s| matches!(
                s,
                ClientCommand::CastSpell {
                    spell_id: MIND_VISION,
                    target: None
                }
            )),
            "the far cast goes out with no target"
        );
        let unit = c.world().spawn(Guid(UNHELD)).id();
        c.dies(unit, UNHELD, ObjectType::Player);
        assert!(c.sent().iter().any(|s| matches!(
            s,
            ClientCommand::CancelCast {
                spell_id: MIND_VISION
            }
        )));
        assert_eq!(c.in_flight(), None);
    }

    /// The cursor's click binds the clicked unit into the record's target (`BindTarget`,
    /// `0x6e5cdc`), and that unit's death cancels the cast.
    #[test]
    fn a_clicked_targets_death_cancels_the_cast_it_bound() {
        let mut c = client();
        let mob = c.mob;
        c.cast(FIREBALL);
        assert_eq!(c.in_flight(), Some(FIREBALL), "the cursor holds the slot");
        c.world()
            .run_system_once(|mut ladder: crate::spell::CastLadder| {
                ladder.commit_targeted(
                    FIREBALL,
                    crate::spell::CastCommit::Spell,
                    crate::spell::TargetedBind::Unit(MOB),
                );
            })
            .expect("the commit runs on the built client");
        assert!(c.sent().iter().any(|s| matches!(
            s,
            ClientCommand::CastSpell {
                spell_id: FIREBALL,
                target: Some(MOB)
            }
        )));
        c.dies(mob, MOB, ObjectType::Unit);
        assert!(c
            .sent()
            .iter()
            .any(|s| matches!(s, ClientCommand::CancelCast { spell_id: FIREBALL })));
    }

    /// `0x6058e5`-`0x605906`: the selection's death deselects (`0x493540(0, 0)` at `0x605901`)
    /// and then stops auto-repeat (`0x6ea080` at `0x605906`): `CMSG_SET_SELECTION` 0, then
    /// `CMSG_CANCEL_AUTO_REPEAT_SPELL`, the key cleared and `STOP_AUTOREPEAT_SPELL` queued.
    #[test]
    fn the_selections_death_deselects_and_then_stops_auto_repeat() {
        let mut c = client();
        let mob = c.mob;
        c.select(MOB, mob);
        c.world().resource_mut::<AutoRepeatActive>().0 = Some(SHOOT);
        c.dies(mob, MOB, ObjectType::Unit);
        let sent = c.sent();
        assert!(
            matches!(
                sent[..],
                [
                    ClientCommand::SetSelection { guid: 0 },
                    ClientCommand::CancelAutoRepeat
                ]
            ),
            "{sent:?}"
        );
        assert_eq!(c.world().resource::<Selection>().guid, None);
        assert_eq!(c.world().resource::<Selection>().last, Some(MOB));
        assert_eq!(c.world().resource::<AutoRepeatActive>().0, None);
        assert!(c
            .reports()
            .iter()
            .any(|r| matches!(r, CastReport::AutoRepeat(false))));
    }

    /// `ReleaseTarget 0x493910` stops our attack (`0x493a08`) before it sends
    /// `CMSG_SET_SELECTION` 0 (`0x493a2a`): the selection's death while we swing at it sends the
    /// stop, the deselect and then the repeat's cancel (`0x605906`).
    #[test]
    fn the_selections_death_mid_swing_stops_before_it_deselects() {
        let mut c = client();
        let (me, mob) = (c.me, c.mob);
        c.select(MOB, mob);
        c.world().entity_mut(me).insert(Engaged(MOB));
        c.world().resource_mut::<AutoRepeatActive>().0 = Some(SHOOT);
        c.dies(mob, MOB, ObjectType::Unit);
        let sent = c.sent();
        assert!(
            matches!(
                sent[..],
                [
                    ClientCommand::AttackStop,
                    ClientCommand::SetSelection { guid: 0 },
                    ClientCommand::CancelAutoRepeat
                ]
            ),
            "{sent:?}"
        );
    }

    /// A unit that is not the selection and names no record dies with no deselect and no stop.
    #[test]
    fn another_units_death_touches_neither_the_selection_nor_the_repeat() {
        let mut c = client();
        let (mob, other) = (c.mob, c.other);
        c.select(MOB, mob);
        c.world().resource_mut::<AutoRepeatActive>().0 = Some(SHOOT);
        c.dies(other, OTHER, ObjectType::Unit);
        assert!(c.sent().is_empty());
        assert_eq!(c.world().resource::<Selection>().guid, Some(MOB));
        assert_eq!(c.world().resource::<AutoRepeatActive>().0, Some(SHOOT));
    }

    /// `0x605896`-`0x6058a8`: the dying unit's `+0xc48` to `+0xc54` zeroed, its attack target
    /// ([`Engaged`]) and, for the active player, the stop in flight (`[+0xc54]`), which the
    /// attack stop's echo clears too (`0x624e40` at `0x624e6b`).
    #[test]
    fn a_death_and_the_attack_stop_echo_clear_the_attack_words() {
        let mut c = client();
        let (me, mob) = (c.me, c.mob);
        c.world().entity_mut(mob).insert(Engaged(ME));
        c.dies(mob, MOB, ObjectType::Unit);
        assert!(c.world().get::<Engaged>(mob).is_none(), "the mob's attack");

        let stop = |c: &mut Client| {
            c.world()
                .run_system_once(|mut seam: AttackSeam| seam.stop())
                .expect("the stop runs on the built client");
            assert!(
                c.world().resource::<AttackStopInFlight>().of(me),
                "the stop's send sets it (`0x6243b2`)"
            );
        };
        c.world().entity_mut(me).insert(Engaged(MOB));
        stop(&mut c);
        c.dies(me, ME, ObjectType::Player);
        assert!(!c.world().resource::<AttackStopInFlight>().of(me), "death");
        assert!(c.world().get::<Engaged>(me).is_none());

        c.world().entity_mut(me).insert(Engaged(MOB));
        stop(&mut c);
        crate::net::handlers::dispatch(
            c.world(),
            crate::net::wire(vec![benilla_protocol::SessionEvent::AttackStop {
                attacker: ME,
                victim: MOB,
            }]),
        );
        assert!(
            !c.world().resource::<AttackStopInFlight>().of(me),
            "the echo"
        );
    }

    /// `CGPlayer_C::OnDied 0x5e99b0`, run for a player (`0x605882` → `0x60588d`): another player
    /// than the active one (`0x5e99d8`-`0x5e99e4`) who holds one of our four party slots
    /// (`IsPartyMember 0x4918d0` → `0x4e7f70`, the slots at `0xbc6f48`) shows `ERR_PLAYER_DIED_S`,
    /// message `0x4c`, with `GetUnitName`'s name (`0x609210` at `0x5e9a08`, `0x496720` at
    /// `0x5e9a10`): the name cache's, `UNKNOWNOBJECT` on a miss. A raid member in another
    /// subgroup holds no slot; a stranger, a creature and our own death show nothing.
    #[test]
    fn a_party_members_death_names_them_and_no_one_elses_does() {
        use crate::ui_action::{FillArg, UiError, UiErrorKeys};
        use benilla_protocol::messages::GroupMemberEntry;
        const MATE: u64 = 0x21;
        const NAMELESS: u64 = 0x22;
        const FAR: u64 = 0x23;
        const STRANGER: u64 = 0x24;
        let mut c = client();
        let mut players = Vec::new();
        for guid in [MATE, NAMELESS, FAR, STRANGER] {
            let store = ObjectStore(
                ObjectFields::from_pairs(&[
                    (2, 0x19),
                    (FIELD_UNIT_HEALTH, 100),
                    (FIELD_UNIT_MAXHEALTH, 100),
                ])
                .into_created(ObjectType::Player),
            );
            let e = c.world().spawn((Guid(guid), store)).id();
            c.world().resource_mut::<GuidIndex>().0.insert(guid, e);
            players.push(e);
        }
        let entry = |name: &str, guid: u64, subgroup: u8| GroupMemberEntry {
            name: name.into(),
            guid,
            status: 1,
            flags: subgroup,
        };
        c.world()
            .resource_mut::<crate::ui_party::GroupState>()
            .apply_list(
                crate::ui_party::GROUPTYPE_RAID,
                0,
                vec![
                    entry("Mate", MATE, 0),
                    entry("Nameless", NAMELESS, 0),
                    entry("Far", FAR, 1),
                ],
                ME,
                None,
                Some(ME),
            );
        let mut names = c.world().resource_mut::<crate::names::NameCache>();
        for (guid, name) in [(MATE, "Mate"), (FAR, "Far"), (STRANGER, "Stranger")] {
            names.insert_player(guid, name.into(), None);
        }
        let _ = c.world().resource::<UiErrorKeys>().take();
        let lines = |c: &mut Client, entity, guid, kind| {
            // Each one-shot reads every edge still buffered: only this death's.
            c.world().resource_mut::<Messages<FieldChanged>>().clear();
            c.dies(entity, guid, kind);
            c.world().resource::<UiErrorKeys>().take()
        };
        assert_eq!(
            lines(&mut c, players[0], MATE, ObjectType::Player),
            [UiError::s("ERR_PLAYER_DIED_S", "Mate")]
        );
        assert_eq!(
            lines(&mut c, players[1], NAMELESS, ObjectType::Player),
            [UiError::args(
                "ERR_PLAYER_DIED_S",
                vec![FillArg::UnknownObject]
            )],
            "a name the cache misses"
        );
        assert_eq!(
            lines(&mut c, players[2], FAR, ObjectType::Player),
            [],
            "another subgroup"
        );
        assert_eq!(
            lines(&mut c, players[3], STRANGER, ObjectType::Player),
            [],
            "no groupmate"
        );
        let (mob, me) = (c.mob, c.me);
        assert_eq!(lines(&mut c, mob, MOB, ObjectType::Unit), [], "a creature");
        assert_eq!(lines(&mut c, me, ME, ObjectType::Player), [], "our own");
    }

    /// A possessed unit's attack, latched: charmed by us, flagged possessed, our view on it, as
    /// `0x5ee5a0` answers it.
    fn possessing_an_attacker(c: &mut Client) {
        const PET: u64 = 0xF140_0000_0000_0099;
        let world = c.world();
        let pet = world
            .spawn((
                Guid(PET),
                ObjectStore(ObjectFields::from_pairs(&[
                    (2, 0x9),
                    (FIELD_UNIT_HEALTH, 100),
                    (10, ME as u32),
                    (FIELD_UNIT_FLAGS, crate::target::UNIT_FLAG_POSSESSED),
                ])),
            ))
            .id();
        world.resource_mut::<GuidIndex>().0.insert(PET, pet);
        let me = c.me;
        let world = c.world();
        world
            .get_mut::<ObjectStore>(me)
            .unwrap()
            .0
            .merge(ObjectFields::from_pairs(&[
                (712, PET as u32),
                (713, (PET >> 32) as u32),
            ]));
        world
            .resource_mut::<crate::ui_pet::PetBar>()
            .spells
            .pet_guid = PET;
        world.resource_mut::<crate::ui_pet::PetAttackLatch>().0 = Some(PET);
    }

    /// The selection's death deselects (`0x605901`), whose teardown calls a possessed unit off
    /// (`0x493a18` → `0x4bd650`): `PetStopAttack` signals `PET_BAR_UPDATE` (`0x4bd6b4`) ahead of
    /// `PLAYER_TARGET_CHANGED` (`0x493a73`), each heard once. The listeners run on the deaths lane,
    /// so the listener's packet follows the deselect's `CMSG_SET_SELECTION` 0.
    #[test]
    fn the_selections_death_signals_its_pet_stop_and_its_change_in_place() {
        let mut c = client();
        // In the world before the bar: the entry's pet reset drops a bar (`0x4bc6f0`).
        crate::script_calls::insert_test_vm(c.world());
        crate::ui_unit::run_feed_units_once(c.world());
        possessing_an_attacker(&mut c);
        let mob = c.mob;
        c.select(MOB, mob);
        crate::script_calls::with_hosted_vm(c.world(), |s| {
            s.expect("a VM").run(
                r#"HEARD = {}
                local r = CreateFrame("Frame")
                r:RegisterEvent("PET_BAR_UPDATE")
                r:RegisterEvent("PLAYER_TARGET_CHANGED")
                r:SetScript("OnEvent", function()
                    table.insert(HEARD, event)
                    if event == "PET_BAR_UPDATE" then RandomRoll(1, 100) end
                end)"#,
            )
        })
        .expect("the listener is in");
        let _ = c.sent();
        c.dies(mob, MOB, ObjectType::Unit);
        c.lanes();
        let wire: Vec<String> = c
            .sent()
            .into_iter()
            .filter_map(|cmd| match cmd {
                ClientCommand::PetStopAttack { .. } => Some("PetStopAttack".to_string()),
                ClientCommand::RandomRoll { .. } => Some("RandomRoll".to_string()),
                ClientCommand::SetSelection { guid } => Some(format!("SetSelection {guid:#x}")),
                _ => None,
            })
            .collect();
        assert_eq!(wire, ["PetStopAttack", "SetSelection 0x0", "RandomRoll"]);
        crate::ui_unit::run_feed_units_once(c.world());
        let heard: Vec<String> = crate::script_calls::eval_in_vm(c.world(), "return HEARD");
        assert_eq!(heard, ["PET_BAR_UPDATE", "PLAYER_TARGET_CHANGED"]);
    }
}
