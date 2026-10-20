//! The melee impact frame. At receive a resolved attacker caches the swing record
//! (`attacker+0xd70`, one slot, overwritten) and plays its clip (`0x5fe2f0`); the victim's
//! feedback fires once, when the clip crosses a `$AH0`-`$AH3` or `$CAH` key (`0x6247d0` →
//! `0x624530`). `$HIT` is no impact key: it re-fires only the flinch and rides hit-reaction clips.
//! `$CPP` (`0x624a01`) plays the victim's defense clip and clears `HitInfo & 0x2` in the record,
//! so no flinch or blood, unless an impact key consumed the record first. A whiff plays the rest
//! of the attacker's swing at half speed (`0x624ca0` → `0x712910(bone 0, 0.5)`). A superseding
//! swing or `SMSG_ATTACKSTOP` flushes the old record as floating text only, a `text_only`
//! [`SwingImpact`], in its packet's handler ([`receive_swing`], [`stop_swing`]). A clip with no
//! impact key never fires: there is no timer fallback. Each feedback queues the victim's
//! `UNIT_COMBAT` where it fires ([`ImpactSinks::fire`]).

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use super::events::AnimSoundEvent;
use super::SwingMessage;
use crate::net::{Guid, ObjectStore};
use crate::ui_chat::ChatLog;

/// The victim dispatcher's lootable gate (`0x624552`, `UNIT_DYNAMIC_FLAGS` bit `0x1`): `0x624530`
/// bails before every consequence it owns, the floating number (`0x624566`) included, so a corpse
/// you can loot shows no hits. The supersede and attack-stop flushes call the number emitter
/// `0x6243e0` directly (`0x625893`, `0x624ea7`) and the whiff slow-down (`0x624ca0`) is outside
/// the dispatcher, so only a full dispatch is gated.
pub(crate) fn lootable_victim(stores: &Query<&ObjectStore>, victim: Option<Entity>) -> bool {
    victim.is_some_and(|v| stores.get(v).is_ok_and(|s| s.0.unit_lootable()))
}

/// A melee swing's impact moment, at the clip's attack-hit key or a flush.
#[derive(Message, Clone, Copy)]
pub(crate) struct SwingImpact {
    pub(crate) swing: SwingMessage,
    /// A supersede or attack-stop flush (`0x624e40`, the `0x14a` overwrite): floating text only.
    pub(crate) text_only: bool,
    /// The `$AH0`-`$AH3` digit: picks the attacker's `CreatureSoundData.CustomAttack` column and
    /// latches `SWINGNOHITSOUND` in place of the weapon-impact sound (`0x6247d0`). `None` for
    /// `$CAH`, the receive-time fallback and flushes.
    pub(crate) natural: Option<u8>,
    /// The firing tag's world point, which the reference carries from `0x624862` into both
    /// weapon-sound legs (`0x6248ef`, `0x624950`); `None` when no tag fired.
    pub(crate) pos: Option<Vec3>,
}

/// Where a swing's victim feedback goes: the [`SwingImpact`] its float, blood, sound and flinch
/// read, and its `UNIT_COMBAT`, queued in the chat drain at once
/// ([`crate::ui_unit::melee_unit_combat`]).
#[derive(SystemParam)]
pub(crate) struct ImpactSinks<'w, 's> {
    out: MessageWriter<'w, SwingImpact>,
    log: ResMut<'w, ChatLog>,
    guids: Query<'w, 's, &'static Guid>,
    pub(crate) stores: Query<'w, 's, &'static ObjectStore>,
}

impl ImpactSinks<'_, '_> {
    /// One feedback, at the moment its caller of `0x6243e0` reaches it.
    pub(crate) fn fire(&mut self, impact: SwingImpact) {
        let victim = impact
            .swing
            .victim
            .and_then(|v| Some((self.guids.get(v).ok()?.0, self.stores.get(v).ok()?)));
        crate::ui_unit::melee_unit_combat(&mut self.log, &impact.swing, victim);
        self.out.write(impact);
    }
}

/// A held attacker's swing at receive (`0x62583f`-`0x6258d5`): its one slot takes the record, and
/// the record it held flushes first as text only (`0x625893`), before the packet's line.
pub(crate) fn receive_swing(
    swing: SwingMessage,
    pending: &mut PendingImpacts,
    sinks: &mut ImpactSinks,
) {
    let old = pending.0.insert(
        swing.attacker,
        PendingSwing {
            swing,
            defended: false,
        },
    );
    if let Some(old) = old {
        if benilla_assets::trace::enabled() {
            benilla_assets::trace::line(
                "fct",
                &format!(
                    "flush supersede atk={:?} dmg={}",
                    swing.attacker, old.swing.damage
                ),
            );
        }
        sinks.fire(SwingImpact {
            swing: old.swing,
            text_only: true,
            natural: None,
            pos: None,
        });
    }
}

/// `SMSG_ATTACKSTOP`'s `0x624e40` on the attacker: its pending record flushes as text only
/// (`0x624ea7`) and the slot clears; death and stun arrive as this packet.
pub(crate) fn stop_swing(attacker: Entity, pending: &mut PendingImpacts, sinks: &mut ImpactSinks) {
    if let Some(p) = pending.0.remove(&attacker) {
        if benilla_assets::trace::enabled() {
            benilla_assets::trace::line(
                "fct",
                &format!("flush stop atk={:?} dmg={}", attacker, p.swing.damage),
            );
        }
        sinks.fire(SwingImpact {
            swing: p.swing,
            text_only: true,
            natural: None,
            pos: None,
        });
    }
}

/// The victim's defense clip (`$CPP`, `0x624a01`), resolved in the driver by
/// [`super::select::defense_anim`], which requires the victim alive, as the client does.
#[derive(Message, Clone, Copy)]
pub(crate) struct DefenseAnim {
    pub(crate) victim: Entity,
    pub(crate) victim_state: u32,
}

/// The whiff slow-down: the attacker's swing runs at half speed for its remainder (`0x712910`).
#[derive(Message, Clone, Copy)]
pub(crate) struct SwingSlowdown(pub(crate) Entity);

/// A swing awaiting its clip's keys (`attacker+0xd70`); `defended` latches the one `$CPP`, so a
/// variation wrap cannot defend it twice.
pub(crate) struct PendingSwing {
    pub(crate) swing: SwingMessage,
    defended: bool,
}

/// Pending swing records, one per attacker (`attacker+0xd70`).
#[derive(Resource, Default)]
pub(crate) struct PendingImpacts(pub(crate) bevy::ecs::entity::EntityHashMap<PendingSwing>);

/// `0x6247d0`'s attack-hit tags: `$AH0`-`$AH3`, or `$CAH`, which every character attack clip
/// authors beside an inert `$HIT`; never `$HIT`. The inner value is [`SwingImpact::natural`].
fn impact_tag(ident: &[u8; 4]) -> Option<Option<u8>> {
    match ident {
        b"$AH0" => Some(Some(0)),
        b"$AH1" => Some(Some(1)),
        b"$AH2" => Some(Some(2)),
        b"$AH3" => Some(Some(3)),
        b"$CAH" => Some(None),
        _ => None,
    }
}

/// The whiff gate `0x624ca0`: miss, dodge or evade; a parry or block still makes contact.
fn is_whiff(victim_state: u32) -> bool {
    matches!(victim_state, 0 | 2 | 6)
}

/// Fire cached swings on their impact keys. Runs after [`super::events::fire_anim_events`] so a
/// key fires, and queues its `UNIT_COMBAT`, the frame it is crossed.
pub(super) fn route_swing_impacts(
    mut events: MessageReader<AnimSoundEvent>,
    mut pending: ResMut<PendingImpacts>,
    entities: Query<(), With<crate::net::NetEntity>>,
    mut sinks: ImpactSinks,
    mut defenses: MessageWriter<DefenseAnim>,
    mut slows: MessageWriter<SwingSlowdown>,
) {
    for ev in events.read() {
        if let Some(natural) = impact_tag(&ev.ident) {
            // The key consumes the record (`0x6247d0` clears it after the dispatch).
            if let Some(p) = pending.0.remove(&ev.entity) {
                if benilla_assets::trace::enabled() {
                    benilla_assets::trace::line(
                        "fct",
                        &format!(
                            "impact tag={} atk={:?} dmg={}",
                            String::from_utf8_lossy(&ev.ident),
                            ev.entity,
                            p.swing.damage
                        ),
                    );
                }
                if is_whiff(p.swing.victim_state) {
                    // Not behind the lootable gate: the slow-down is outside the dispatcher.
                    slows.write(SwingSlowdown(ev.entity));
                }
                // A lootable victim takes nothing, not even the number; the record is consumed
                // all the same, as `0x6247d0` clears it whatever the dispatcher does.
                if lootable_victim(&sinks.stores, p.swing.victim) {
                    continue;
                }
                sinks.fire(SwingImpact {
                    swing: p.swing,
                    text_only: false,
                    natural,
                    pos: ev.pos,
                });
            } else if benilla_assets::trace::enabled() {
                benilla_assets::trace::line(
                    "fct",
                    &format!(
                        "impact tag={} atk={:?} NO-PENDING",
                        String::from_utf8_lossy(&ev.ident),
                        ev.entity
                    ),
                );
            }
        } else if &ev.ident == b"$CPP" {
            // The defense edits the cached record and never consumes it.
            if let Some(p) = pending.0.get_mut(&ev.entity) {
                if !p.defended {
                    p.defended = true;
                    if matches!(p.swing.victim_state, 2 | 3 | 5 | 8) {
                        if let Some(victim) = p.swing.victim {
                            defenses.write(DefenseAnim {
                                victim,
                                victim_state: p.swing.victim_state,
                            });
                        }
                        // `and [esi+0xd80],-3`: a defended hit never flinches or bleeds.
                        p.swing.hit_info &= !0x2;
                    }
                }
            }
        }
    }
    // A despawned attacker's record drops without a flush, as the client's destructor clears it.
    if !pending.0.is_empty() {
        pending.0.retain(|e, _| entities.contains(*e));
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::message::MessageCursor;
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    fn swing(attacker: Entity, victim: Option<Entity>, victim_state: u32) -> SwingMessage {
        SwingMessage {
            attacker,
            victim,
            hit_info: 0x2,
            victim_state,
            damage: if victim_state == 1 { 42 } else { 0 },
            displayed: true,
            seq: 1,
        }
    }

    fn app() -> App {
        let mut app = App::new();
        app.init_resource::<PendingImpacts>();
        app.init_resource::<ChatLog>();
        app.add_message::<AnimSoundEvent>();
        app.add_message::<SwingImpact>();
        app.add_message::<DefenseAnim>();
        app.add_message::<SwingSlowdown>();
        app.add_systems(Update, route_swing_impacts);
        app
    }

    /// `SMSG_ATTACKERSTATEUPDATE`'s receive for a held attacker, as its handler runs it.
    fn receive(app: &mut App, s: SwingMessage) {
        app.world_mut()
            .run_system_once(
                move |mut pending: ResMut<PendingImpacts>, mut sinks: ImpactSinks| {
                    receive_swing(s, &mut pending, &mut sinks);
                },
            )
            .unwrap();
        app.update();
    }

    /// `SMSG_ATTACKSTOP` on `attacker`, as its handler runs it.
    fn stop(app: &mut App, attacker: Entity) {
        app.world_mut()
            .run_system_once(
                move |mut pending: ResMut<PendingImpacts>, mut sinks: ImpactSinks| {
                    stop_swing(attacker, &mut pending, &mut sinks);
                },
            )
            .unwrap();
        app.update();
    }

    fn unit(app: &mut App) -> Entity {
        app.world_mut()
            .spawn(crate::net::NetEntity {
                kind: benilla_protocol::EntityKind::Unit,
                display_id: None,
                scale: 1.0,
            })
            .id()
    }

    fn tag(app: &mut App, entity: Entity, ident: [u8; 4]) {
        app.world_mut().write_message(AnimSoundEvent {
            entity,
            ident,
            data: 0,
            anim_id: 0,
            pos: None,
        });
        app.update();
    }

    /// The impacts since the last call; one cursor per test, as a fresh one would re-read the
    /// previous frame.
    #[allow(clippy::type_complexity)]
    fn drain(
        app: &mut App,
        cursor: &mut MessageCursor<SwingImpact>,
    ) -> Vec<(u32, u32, bool, Option<u8>)> {
        let msgs = app.world().resource::<Messages<SwingImpact>>();
        cursor
            .read(msgs)
            .map(|i| {
                (
                    i.swing.hit_info,
                    i.swing.victim_state,
                    i.text_only,
                    i.natural,
                )
            })
            .collect()
    }

    fn drain_defenses(app: &mut App, cursor: &mut MessageCursor<DefenseAnim>) -> Vec<u32> {
        let msgs = app.world().resource::<Messages<DefenseAnim>>();
        cursor.read(msgs).map(|d| d.victim_state).collect()
    }

    fn drain_slows(app: &mut App, cursor: &mut MessageCursor<SwingSlowdown>) -> usize {
        let msgs = app.world().resource::<Messages<SwingSlowdown>>();
        cursor.read(msgs).count()
    }

    /// `UNIT_DYNAMIC_FLAGS`'s UpdateField index; bit `0x1` is LOOTABLE.
    const FIELD_DYNAMIC_FLAGS: u16 = 143;

    /// Give `entity` a store carrying `flags` in `UNIT_DYNAMIC_FLAGS`.
    fn dynamic_flags(app: &mut App, entity: Entity, flags: u32) {
        app.world_mut()
            .entity_mut(entity)
            .insert(crate::net::ObjectStore(
                benilla_protocol::messages::ObjectFields::from_pairs(&[(
                    FIELD_DYNAMIC_FLAGS,
                    flags,
                )]),
            ));
    }

    /// The lootable gate (`0x624552`) drops the whole dispatch; the tag still consumes the record.
    #[test]
    fn a_lootable_victim_takes_no_full_dispatch() {
        let mut app = app();
        let (attacker, victim) = (unit(&mut app), unit(&mut app));
        dynamic_flags(&mut app, victim, 0x1);
        let mut cursor = MessageCursor::<SwingImpact>::default();
        receive(&mut app, swing(attacker, Some(victim), 1));
        drain(&mut app, &mut cursor);

        tag(&mut app, attacker, *b"$CAH");
        assert!(
            drain(&mut app, &mut cursor).is_empty(),
            "a lootable victim gets nothing — not even the number",
        );
        assert!(
            !app.world()
                .resource::<PendingImpacts>()
                .0
                .contains_key(&attacker),
            "the record is still consumed by the tag",
        );
    }

    #[test]
    fn a_non_lootable_victim_still_takes_the_full_dispatch() {
        let mut app = app();
        let (attacker, victim) = (unit(&mut app), unit(&mut app));
        dynamic_flags(&mut app, victim, 0x20); // dead-looking, not lootable
        let mut cursor = MessageCursor::<SwingImpact>::default();
        receive(&mut app, swing(attacker, Some(victim), 1));
        drain(&mut app, &mut cursor);

        tag(&mut app, attacker, *b"$CAH");
        assert_eq!(drain(&mut app, &mut cursor), vec![(0x2, 1, false, None)]);
    }

    /// The flushes call the number emitter `0x6243e0` directly (`0x624ea7`, `0x625893`).
    #[test]
    fn a_lootable_victim_still_gets_the_supersede_and_stop_flushes() {
        let mut app = app();
        let (attacker, victim) = (unit(&mut app), unit(&mut app));
        dynamic_flags(&mut app, victim, 0x1);
        let mut cursor = MessageCursor::<SwingImpact>::default();

        // The second swing supersedes the first, which flushes as text only.
        receive(&mut app, swing(attacker, Some(victim), 1));
        drain(&mut app, &mut cursor);
        receive(&mut app, swing(attacker, Some(victim), 1));
        assert_eq!(
            drain(&mut app, &mut cursor),
            vec![(0x2, 1, true, None)],
            "the superseded record still texts, gate or no gate",
        );

        // `SMSG_ATTACKSTOP` flushes the survivor the same way.
        stop(&mut app, attacker);
        assert_eq!(drain(&mut app, &mut cursor), vec![(0x2, 1, true, None)]);
    }

    /// The whiff slow-down (`0x624ca0`) is not the dispatcher's, so the gate does not stop it.
    #[test]
    fn the_whiff_slowdown_survives_the_lootable_gate() {
        let mut app = app();
        let (attacker, victim) = (unit(&mut app), unit(&mut app));
        dynamic_flags(&mut app, victim, 0x1);
        let mut impacts = MessageCursor::<SwingImpact>::default();
        let mut slows = MessageCursor::<SwingSlowdown>::default();
        receive(&mut app, swing(attacker, Some(victim), 2)); // dodged
        drain(&mut app, &mut impacts);
        drain_slows(&mut app, &mut slows);

        tag(&mut app, attacker, *b"$CAH");
        assert_eq!(
            drain_slows(&mut app, &mut slows),
            1,
            "the whiff still slows"
        );
        assert!(
            drain(&mut app, &mut impacts).is_empty(),
            "but nothing dispatches"
        );
    }

    /// The first `$AH`/`$CAH` fires a swing in full, once; `$HIT` and `$CSS` never do.
    #[test]
    fn impact_fires_full_on_ah_or_cah_never_on_hit() {
        let mut app = app();
        let mut cursor = MessageCursor::<SwingImpact>::default();
        let a = unit(&mut app);
        receive(&mut app, swing(a, None, 1));
        tag(&mut app, a, *b"$CSS");
        tag(&mut app, a, *b"$HIT");
        assert_eq!(
            drain(&mut app, &mut cursor),
            vec![],
            "$HIT must not fire it"
        );
        tag(&mut app, a, *b"$CAH");
        assert_eq!(drain(&mut app, &mut cursor), vec![(0x2, 1, false, None)]);
        app.update();
        assert!(drain(&mut app, &mut cursor).is_empty(), "fires once");
    }

    /// `$CPP` first: the defense fires once, the record loses `0x2`, and the impact still fires.
    #[test]
    fn cpp_defends_clears_flinch_then_impact_texts() {
        let mut app = app();
        let mut cursor = MessageCursor::<SwingImpact>::default();
        let mut d_cursor = MessageCursor::<DefenseAnim>::default();
        let a = unit(&mut app);
        let v = unit(&mut app);
        receive(&mut app, swing(a, Some(v), 3));
        tag(&mut app, a, *b"$CPP");
        assert_eq!(drain_defenses(&mut app, &mut d_cursor), vec![3]);
        tag(&mut app, a, *b"$CPP");
        assert_eq!(
            drain_defenses(&mut app, &mut d_cursor),
            Vec::<u32>::new(),
            "defended latch: a second $CPP is a no-op"
        );
        tag(&mut app, a, *b"$AH0");
        assert_eq!(
            drain(&mut app, &mut cursor),
            vec![(0x0, 3, false, Some(0))],
            "the impact fires with the flinch bit cleared"
        );
    }

    #[test]
    fn impact_first_consumes_no_late_defense() {
        let mut app = app();
        let mut cursor = MessageCursor::<SwingImpact>::default();
        let mut d_cursor = MessageCursor::<DefenseAnim>::default();
        let a = unit(&mut app);
        let v = unit(&mut app);
        receive(&mut app, swing(a, Some(v), 5));
        tag(&mut app, a, *b"$CAH");
        assert_eq!(drain(&mut app, &mut cursor), vec![(0x2, 5, false, None)]);
        tag(&mut app, a, *b"$CPP");
        assert_eq!(
            drain_defenses(&mut app, &mut d_cursor),
            Vec::<u32>::new(),
            "consumed record: no late defense"
        );
    }

    #[test]
    fn whiff_slows_on_impact_tag() {
        let mut app = app();
        let mut s_cursor = MessageCursor::<SwingSlowdown>::default();
        let a = unit(&mut app);
        let v = unit(&mut app);
        receive(&mut app, swing(a, Some(v), 2));
        tag(&mut app, a, *b"$AH1");
        assert_eq!(drain_slows(&mut app, &mut s_cursor), 1, "a dodge slows");
        receive(&mut app, swing(a, Some(v), 1));
        tag(&mut app, a, *b"$AH1");
        assert_eq!(drain_slows(&mut app, &mut s_cursor), 0, "a hit never slows");
    }

    /// The superseding swing still waits for its own key and fires in full.
    #[test]
    fn superseded_swing_flushes_text_only() {
        let mut app = app();
        let mut cursor = MessageCursor::<SwingImpact>::default();
        let a = unit(&mut app);
        receive(&mut app, swing(a, None, 1));
        receive(&mut app, swing(a, None, 5));
        assert_eq!(drain(&mut app, &mut cursor), vec![(0x2, 1, true, None)]);
        tag(&mut app, a, *b"$AH1");
        assert_eq!(drain(&mut app, &mut cursor), vec![(0x2, 5, false, Some(1))]);
    }

    /// A victim the client holds as a unit, with its guid.
    fn held_unit(app: &mut App, guid: u64) -> Entity {
        let e = unit(app);
        let store = benilla_protocol::messages::ObjectFields::from_pairs(&[(2, 0x9)]);
        app.world_mut()
            .entity_mut(e)
            .insert((Guid(guid), ObjectStore(store)));
        e
    }

    fn kinds(app: &App) -> Vec<&'static str> {
        app.world().resource::<ChatLog>().pending_kinds()
    }

    /// The key's `UNIT_COMBAT` is queued the frame the key is crossed (`0x6249bb` → `0x624530` →
    /// `0x6243e0`), behind what is queued already and ahead of the next packets' lines.
    #[test]
    fn a_key_queues_its_unit_combat_the_frame_it_is_crossed() {
        let mut app = app();
        let (a, v) = (unit(&mut app), held_unit(&mut app, 7));
        receive(&mut app, swing(a, Some(v), 1));
        app.world_mut()
            .resource_mut::<ChatLog>()
            .push_xp_gain(0, 10, 10);
        tag(&mut app, a, *b"$CAH");
        assert_eq!(kinds(&app), ["xp", "unit combat"]);
        let flashed = app.world().resource::<ChatLog>().pending_unit_combat();
        assert_eq!(flashed[0].guid, 7);
    }

    /// The flushes queue theirs in the packet's handler (`0x625893`, `0x624ea7`).
    #[test]
    fn a_flush_queues_its_unit_combat_at_the_packet() {
        let mut app = app();
        let (a, v) = (unit(&mut app), held_unit(&mut app, 7));
        receive(&mut app, swing(a, Some(v), 1));
        assert!(kinds(&app).is_empty(), "the record waits for its key");
        receive(&mut app, swing(a, Some(v), 1));
        assert_eq!(kinds(&app), ["unit combat"], "superseded");
        stop(&mut app, a);
        assert_eq!(kinds(&app), ["unit combat", "unit combat"], "stopped");
    }

    /// An untagged record with no stop waits: there is no timer fallback.
    #[test]
    fn attack_stop_flushes_text_only() {
        let mut app = app();
        let mut cursor = MessageCursor::<SwingImpact>::default();
        let a = unit(&mut app);
        receive(&mut app, swing(a, None, 1));
        app.update();
        assert!(drain(&mut app, &mut cursor).is_empty(), "no timer fallback");
        stop(&mut app, a);
        assert_eq!(drain(&mut app, &mut cursor), vec![(0x2, 1, true, None)]);
    }
}
