//! The dialog and queue verbs through the whole client's host: each sends, changes its state and
//! signals inside its call, so a chunk's packets go out in call order with another system's verb
//! (`RandomRoll`, `0x48c832`) and the chunk's next line reads what the verb did.

use benilla_protocol::messages::{BattlefieldList, BattlefieldStatus, LootStartRoll};
use benilla_protocol::ObjectFields;
use benilla_ui::script::{
    BattlefieldListView, BattlefieldPositionView, BattlefieldQueueSlot, BattlefieldScores,
    LootRollEntry, LootRollsState, UiScript, UnitGuids,
};
use bevy::prelude::*;
use crossbeam_channel::Receiver;

use super::systems::{client, eval, listen, NPC};
use crate::net::{ClientCommand, Guid, GuidIndex, ObjectStore, SelfGuid, SelfPlayer};

/// Our own guid in these chunks.
const ME: u64 = 0x10;
/// `UNIT_FIELD_HEALTH` and `UNIT_FIELD_MAXHEALTH`.
const HEALTH: u16 = 22;
const MAX_HEALTH: u16 = 28;
/// `PLAYER_FLAGS`, whose bit `0x10` is the ghost.
const PLAYER_FLAGS: u16 = 190;
/// `PLAYER_SELF_RES_SPELL`.
const SELF_RES_SPELL: u16 = 1224;

/// What went out, in order, of the kinds these chunks send.
fn sent(rx: &Receiver<ClientCommand>) -> Vec<String> {
    rx.try_iter()
        .filter_map(|c| {
            Some(match c {
                ClientCommand::RandomRoll { min, .. } => format!("roll {min}"),
                ClientCommand::RepopRequest => "repop".into(),
                ClientCommand::ReclaimCorpse { corpse } => format!("reclaim {corpse:#x}"),
                ClientCommand::ResurrectResponse { caster, accept } => {
                    format!("res {caster:#x} {accept}")
                }
                ClientCommand::SpiritHealerActivate { npc } => format!("spirit healer {npc:#x}"),
                ClientCommand::SelfRes => "self res".into(),
                ClientCommand::AreaSpiritHealerQueue { healer } => {
                    format!("area queue {healer:#x}")
                }
                ClientCommand::CancelAura { spell_id } => format!("cancel aura {spell_id}"),
                ClientCommand::DuelAccepted { arbiter } => format!("duel accept {arbiter:#x}"),
                ClientCommand::DuelCancelled { arbiter } => format!("duel cancel {arbiter:#x}"),
                ClientCommand::CastSpell { spell_id, .. } => format!("cast {spell_id}"),
                ClientCommand::BinderActivate { binder } => format!("bind {binder:#x}"),
                ClientCommand::SummonResponse { summoner } => format!("summon {summoner:#x}"),
                ClientCommand::MeetingStoneLeave => "stone leave".into(),
                ClientCommand::ResetInstances => "reset".into(),
                ClientCommand::TogglePvp => "pvp".into(),
                ClientCommand::LootRoll { roll_type, .. } => format!("loot roll {roll_type}"),
                ClientCommand::BattlemasterJoin {
                    map_id,
                    instance_id,
                    ..
                } => format!("bm join {map_id} {instance_id}"),
                ClientCommand::BattlefieldJoin { map_id, .. } => format!("bf join {map_id}"),
                ClientCommand::BattlefieldList { map_id } => format!("list {map_id}"),
                ClientCommand::RequestBattlefieldScoreData => "score".into(),
                ClientCommand::LeaveBattlefield { map_id } => format!("leave {map_id}"),
                ClientCommand::BattlefieldPort { map_id, accept } => {
                    format!("port {map_id} {accept}")
                }
                ClientCommand::RequestBattlefieldPositions => "positions".into(),
                _ => return None,
            })
        })
        .collect()
}

/// Our player seated with `fields`, in the guid index.
fn seat(app: &mut App, fields: &[(u16, u32)]) {
    let world = app.world_mut();
    world.resource_mut::<SelfGuid>().0 = Some(ME);
    let mut pairs = vec![(2, 0x19), (MAX_HEALTH, 100)];
    pairs.extend_from_slice(fields);
    let me = world
        .spawn((
            Guid(ME),
            SelfPlayer,
            Transform::default(),
            ObjectStore(ObjectFields::from_pairs(&pairs)),
        ))
        .id();
    world.resource_mut::<GuidIndex>().0.insert(ME, me);
}

/// A unit `guid` standing `yards` along +X, in the guid index.
fn unit_at(app: &mut App, guid: u64, yards: f32) {
    let world = app.world_mut();
    let e = world
        .spawn((
            Guid(guid),
            Transform::from_xyz(yards, 0.0, 0.0),
            ObjectStore(ObjectFields::from_pairs(&[
                (2, 0x9),
                (HEALTH, 100),
                (MAX_HEALTH, 100),
            ])),
        ))
        .id();
    world.resource_mut::<GuidIndex>().0.insert(guid, e);
}

fn vm(app: &mut App) -> Mut<'_, UiScript> {
    app.world_mut().non_send_resource_mut::<UiScript>()
}

/// A slot of the queue, as the status handler stores it (`0x4aa850`).
fn status(slot: u32, map_id: u32, status: u32) -> BattlefieldStatus {
    BattlefieldStatus {
        slot,
        map_id,
        bracket: 0,
        instance_id: 0,
        status,
        time_ms: None,
        in_progress: (status == 3).then_some((0, 0)),
        queued: None,
    }
}

/// The battleground verbs send inside their calls (`0x4ab290`, `0x4ab8c0`, `0x4ab3b0`,
/// `0x4ab990`, `0x4abef0`, `0x4abe60`): the join with the stored battlemaster (`0x4a9fea`), the
/// list with the queued slot's map, the port answer, the score ask once per 5000 ms
/// (`0x4aa170`), the positions ask with an active slot and the leave once the board ended
/// (`0x4abe66`), each where the chunk calls it.
#[test]
fn the_battleground_verbs_send_inside_their_calls() {
    const WSG: u32 = 489;
    const AV: u32 = 30;
    let (mut app, rx) = client();
    let world = app.world_mut();
    world
        .resource_mut::<crate::ui_battlefield::Battlefield>()
        .apply_list(BattlefieldList {
            battlemaster: NPC,
            map_id: WSG,
            bracket: 0,
            instances: vec![7],
        });
    let mut queue = world.resource_mut::<crate::ui_dialog_verbs::BattlefieldQueue>();
    queue.apply(status(0, WSG, 3));
    queue.apply(status(1, AV, 1));
    let mut script = vm(&mut app);
    script.set_battlefield_list(BattlefieldListView {
        instances: vec![7],
        ..Default::default()
    });
    let slot = |map_id, status| BattlefieldQueueSlot {
        map_id,
        status,
        ..Default::default()
    };
    script.set_battlefield_queue([slot(WSG, 3), slot(AV, 1), slot(0, 0)], 0);
    script.set_battlefield_scores(BattlefieldScores {
        ended: true,
        ..Default::default()
    });
    eval::<()>(
        &mut app,
        "JoinBattlefield(1) RandomRoll(1, 2) ShowBattlefieldList(2) AcceptBattlefieldPort(2, 1) \
         RandomRoll(2, 3) RequestBattlefieldScoreData() RequestBattlefieldScoreData() \
         RequestBattlefieldPositions() LeaveBattlefield() RandomRoll(3, 4)",
    );
    assert_eq!(
        sent(&rx),
        [
            "bm join 489 7",
            "roll 1",
            "list 30",
            "port 30 true",
            "roll 2",
            "score",
            "positions",
            "leave 489",
            "roll 3",
        ]
    );
}

/// `JoinBattlefield(i, 1)` refuses a group the map's `MaxPlayers` cannot hold inside its call
/// (`0x4a9fa8`): `ERR_GROUP_JOIN_BATTLEGROUND_TOO_MANY` shows, its listeners run, and nothing goes
/// out, before the chunk's next line.
#[test]
fn join_battlefield_refuses_a_large_group_inside_its_call() {
    let (mut app, rx) = client();
    app.world_mut()
        .resource_mut::<crate::ui_battlefield::Battlefield>()
        .apply_list(BattlefieldList {
            battlemaster: NPC,
            map_id: 489,
            bracket: 0,
            instances: vec![7],
        });
    {
        let mut group = app
            .world_mut()
            .resource_mut::<crate::ui_party::GroupState>();
        group.in_group = true;
        group.members = vec![benilla_protocol::messages::GroupMemberEntry {
            name: "Bob".into(),
            guid: 0x22,
            status: 1,
            flags: 0,
        }];
    }
    eval::<()>(
        &mut app,
        "ERR_GROUP_JOIN_BATTLEGROUND_TOO_MANY = 'Your group is too large'",
    );
    listen(
        &mut app,
        &["UI_ERROR_MESSAGE", "UI_INFO_MESSAGE", "CHAT_MSG_SYSTEM"],
        "RandomRoll(9, 9)",
    );
    eval::<()>(&mut app, "JoinBattlefield(1, 1) RandomRoll(1, 2)");
    assert_eq!(sent(&rx), ["roll 9", "roll 1"]);
}

/// `RequestBattlefieldPositions()` with no active slot zeroes the teammate count alone
/// (`0x4abef0` → `0x4aa5c0`, `0x4aa5df`) inside its call and sends nothing: the next line counts
/// no teammate, and the carrier stays, in the engine's copy and the VM's.
#[test]
fn request_battlefield_positions_without_a_battleground_clears_inside_its_call() {
    use benilla_protocol::messages::{BattlefieldPosition, BattlefieldPositions};
    let (mut app, rx) = client();
    let at = |guid| BattlefieldPosition {
        guid,
        x: 1.0,
        y: 2.0,
    };
    app.world_mut()
        .resource_mut::<crate::ui_battlefield_positions::BattlefieldPositions>()
        .apply(BattlefieldPositions {
            players: vec![at(0x9)],
            carrier: Some(at(0xA)),
        });
    vm(&mut app).set_battlefield_positions(
        vec![BattlefieldPositionView {
            uv: (0.5, 0.5),
            name: Some("Bob".into()),
            ..Default::default()
        }],
        Some(benilla_ui::script::BattlefieldFlagView {
            uv: (0.25, 0.25),
            token: None,
        }),
        1.0,
    );
    let n: (i64, i64) = eval(
        &mut app,
        "RequestBattlefieldPositions() \
         return GetNumBattlefieldPositions(), GetNumBattlefieldFlagPositions()",
    );
    assert_eq!(n, (0, 1));
    assert!(sent(&rx).is_empty());
    let carrier = app
        .world()
        .resource::<crate::ui_battlefield_positions::BattlefieldPositions>()
        .carrier()
        .map(|c| c.guid);
    assert_eq!(carrier, Some(0xA));
}

/// The death verbs send inside their calls: `RepopMe` (`0x48a970` → `0x5e0ae0`) while dead or a
/// ghost (`0x5e0ae6` → `0x605f30`), `RetrieveCorpse` with the corpse guid (`0x48d260`), and the resurrect
/// answer (`0x5ded80`), which forgets the offer as it sends (`0x5dedf2`), so a second answer in
/// the chunk finds none (`0x5ded91`).
#[test]
fn the_death_verbs_send_inside_their_calls() {
    const CORPSE: u64 = 0xC0;
    const CASTER: u64 = 0xCA;
    // Dead, a released ghost (health 1, `PLAYER_FLAGS` 0x10: `0x605f3d`-`0x605f59`), alive.
    for (fields, dead) in [
        (&[(HEALTH, 0)][..], true),
        (&[(HEALTH, 1), (PLAYER_FLAGS, 0x10)][..], true),
        (&[(HEALTH, 100)][..], false),
    ] {
        let (mut app, rx) = client();
        seat(&mut app, fields);
        {
            let mut death = app.world_mut().resource_mut::<crate::death::DeathNet>();
            death.corpse_guid = Some(CORPSE);
            death.resurrect = Some(crate::death::ResurrectOffer {
                caster: CASTER,
                name: "Priest".into(),
                sickness: false,
                has_timer: false,
            });
        }
        eval::<()>(
            &mut app,
            "RepopMe() RandomRoll(1, 2) RetrieveCorpse() AcceptResurrect() DeclineResurrect() \
             RandomRoll(2, 3)",
        );
        let expected: &[&str] = if dead {
            &["repop", "roll 1", "reclaim 0xc0", "res 0xca true", "roll 2"]
        } else {
            &["roll 1", "reclaim 0xc0", "res 0xca true", "roll 2"]
        };
        assert_eq!(sent(&rx), expected, "{fields:?}");
    }
}

/// `AcceptXPLoss()` (`0x48d0c0` → `0x5df730(0)`) sends `CMSG_SPIRIT_HEALER_ACTIVATE` inside its
/// call for a ghost (`0x5df74a`) whose remembered healer stands within the interaction range
/// (`0x5df7c1`, `[0xc4c28c]`), and keeps the guid (`0x5df7e4` skips the store), so a second accept
/// sends again.
#[test]
fn accept_xp_loss_sends_inside_its_call_within_range() {
    const HEALER: u64 = 0xF130_0000_0000_0050;
    for (ghost, yards, sends) in [(true, 3.0, true), (true, 10.0, false), (false, 3.0, false)] {
        let (mut app, rx) = client();
        let fields = if ghost {
            vec![(HEALTH, 1), (PLAYER_FLAGS, 0x10)]
        } else {
            vec![(HEALTH, 100)]
        };
        seat(&mut app, &fields);
        unit_at(&mut app, HEALER, yards);
        app.world_mut()
            .resource_mut::<crate::death::DeathNet>()
            .ask_spirit_healer(HEALER);
        eval::<()>(&mut app, "AcceptXPLoss() RandomRoll(1, 2) AcceptXPLoss()");
        let expected: &[&str] = if sends {
            &[
                "spirit healer 0xf130000000000050",
                "roll 1",
                "spirit healer 0xf130000000000050",
            ]
        } else {
            &["roll 1"]
        };
        assert_eq!(sent(&rx), expected, "ghost {ghost}, {yards} yd");
    }
}

/// `UseSoulstone()` (`0x48ad70`) with `PLAYER_SELF_RES_SPELL` set sends `CMSG_SELF_RES` inside its
/// call (`0x48adb6`, `0x48ae05`), with no health test, unlike `HasSoulstone()`.
#[test]
fn use_soulstone_sends_inside_its_call() {
    for health in [0, 100] {
        let (mut app, rx) = client();
        seat(&mut app, &[(HEALTH, health), (SELF_RES_SPELL, 3026)]);
        eval::<()>(&mut app, "UseSoulstone() RandomRoll(1, 2)");
        assert_eq!(sent(&rx), ["self res", "roll 1"], "health {health}");
    }
}

/// A spirit guide `guid` standing `yards` along +X, as the scan's walk sees units.
fn guide_at(app: &mut App, guid: u64, yards: f32) {
    unit_at(app, guid, yards);
    let e = app.world().resource::<GuidIndex>().0[&guid];
    app.world_mut().entity_mut(e).insert(crate::net::NetEntity {
        kind: benilla_protocol::EntityKind::Unit,
        display_id: None,
        scale: 1.0,
    });
}

/// The area spirit healer's two verbs inside their calls, for a ghost: `AcceptAreaSpiritHeal()`
/// runs the scan (`0x492316` → `0x4923b0`), which keeps a cached healer within 22 yd, then sends
/// `CMSG_AREA_SPIRIT_HEALER_QUEUE` with it (`0x49232b`); `CancelAreaSpiritHeal()` runs the aura
/// cancel `0x6e7040(2584)`, its `AREA_SPIRIT_HEALER_OUT_OF_RANGE` listeners ahead of
/// `CMSG_CANCEL_AURA` (`0x6e70c3`).
#[test]
fn the_area_spirit_healer_verbs_send_inside_their_calls() {
    const HEALER: u64 = 0xF130_0000_0000_0A5A;
    let (mut app, rx) = client();
    seat(&mut app, &[(HEALTH, 1), (PLAYER_FLAGS, 0x10)]);
    guide_at(&mut app, HEALER, 5.0);
    let _ = app
        .world_mut()
        .resource_mut::<crate::ui_dialog_verbs::AreaSpiritHealer>()
        .set_healer(Some(HEALER));
    listen(
        &mut app,
        &["AREA_SPIRIT_HEALER_OUT_OF_RANGE"],
        "RandomRoll(9, 9)",
    );
    eval::<()>(
        &mut app,
        "AcceptAreaSpiritHeal() RandomRoll(1, 2) CancelAreaSpiritHeal() RandomRoll(2, 3)",
    );
    assert_eq!(
        sent(&rx),
        [
            "area queue 0xf130000000000a5a",
            "roll 1",
            "roll 9",
            "cancel aura 2584",
            "roll 2",
        ]
    );
}

/// `AcceptAreaSpiritHeal()` sends nothing with no healer cached once its scan ran (`0x492326`):
/// none cached and none in reach, and a cached healer walked past 22 yd mid-wave, which the scan
/// drops (`0x4921c0(0)`), its wave cancelled with `AREA_SPIRIT_HEALER_OUT_OF_RANGE`'s listeners
/// ahead of `CMSG_CANCEL_AURA` (`0x4921fc` → `0x6e7040`), all before the chunk's next line.
#[test]
fn accept_area_spirit_heal_rescans_before_it_sends() {
    const HEALER: u64 = 0xF130_0000_0000_0A5A;
    for cached in [false, true] {
        let (mut app, rx) = client();
        seat(&mut app, &[(HEALTH, 1), (PLAYER_FLAGS, 0x10)]);
        if cached {
            guide_at(&mut app, HEALER, 30.0);
            let mut spirit = app
                .world_mut()
                .resource_mut::<crate::ui_dialog_verbs::AreaSpiritHealer>();
            let _ = spirit.set_healer(Some(HEALER));
            spirit.on_time(HEALER, 30_000, std::time::Instant::now());
        }
        listen(
            &mut app,
            &["AREA_SPIRIT_HEALER_OUT_OF_RANGE"],
            "RandomRoll(9, 9)",
        );
        eval::<()>(&mut app, "AcceptAreaSpiritHeal() RandomRoll(1, 2)");
        let expected: &[&str] = if cached {
            &["roll 9", "cancel aura 2584", "roll 1"]
        } else {
            &["roll 1"]
        };
        assert_eq!(sent(&rx), expected, "cached {cached}");
        assert_eq!(
            app.world()
                .resource::<crate::ui_dialog_verbs::AreaSpiritHealer>()
                .healer(),
            None
        );
    }
}

/// `SMSG_AREA_SPIRIT_HEALER_TIME`'s handler rescans before it arms (`0x492290` → `0x492296`):
/// the cached healer within 22 yd arms the wave; walked past it, the scan drops the healer and the
/// time for it arms nothing (`0x4922a0`).
#[test]
fn the_spirit_healer_time_rescans_before_it_arms() {
    const HEALER: u64 = 0xF130_0000_0000_0A5A;
    for (yards, arms) in [(5.0, true), (30.0, false)] {
        let (mut app, _rx) = client();
        seat(&mut app, &[(HEALTH, 1), (PLAYER_FLAGS, 0x10)]);
        guide_at(&mut app, HEALER, yards);
        let _ = app
            .world_mut()
            .resource_mut::<crate::ui_dialog_verbs::AreaSpiritHealer>()
            .set_healer(Some(HEALER));
        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![benilla_protocol::SessionEvent::AreaSpiritHealerTime {
                healer: HEALER,
                ms: 30_000,
            }],
        );
        let spirit = app
            .world()
            .resource::<crate::ui_dialog_verbs::AreaSpiritHealer>();
        assert_eq!(spirit.armed(), arms, "{yards} yd");
        assert_eq!(spirit.healer(), arms.then_some(HEALER), "{yards} yd");
    }
}

/// `AcceptDuel()` and `CancelDuel()` send with the stored flag guid inside their calls, with no
/// test that a duel exists (`0x4d4830`, `0x4d48b0`: `[0xb73240]` straight into the packet).
#[test]
fn the_duel_answers_send_inside_their_calls() {
    for arbiter in [0xD0, 0] {
        let (mut app, rx) = client();
        app.world_mut()
            .resource_mut::<crate::ui_duel::DuelState>()
            .arbiter = arbiter;
        eval::<()>(&mut app, "AcceptDuel() RandomRoll(1, 2) CancelDuel()");
        assert_eq!(
            sent(&rx),
            [
                format!("duel accept {arbiter:#x}"),
                "roll 1".into(),
                format!("duel cancel {arbiter:#x}"),
            ],
        );
    }
}

/// `StartDuelUnit(unit)` casts the duel spell at the unit's guid inside its call (`0x4d4cbc` →
/// `0x4d4810` → `0x6e5a90`): the learned spell whose first effect is 83 (`[0xb71130]`).
#[test]
fn start_duel_casts_inside_its_call() {
    const DUEL: u32 = 7266;
    const FOE: u64 = 0x30;
    let (mut app, rx) = client();
    seat(&mut app, &[(HEALTH, 100)]);
    let world = app.world_mut();
    let foe = world
        .spawn((
            Guid(FOE),
            Transform::from_xyz(5.0, 0.0, 0.0),
            GlobalTransform::from_translation(Vec3::new(5.0, 0.0, 0.0)),
            ObjectStore(ObjectFields::from_pairs(&[
                (2, 0x19),
                (HEALTH, 100),
                (MAX_HEALTH, 100),
            ])),
        ))
        .id();
    world.resource_mut::<GuidIndex>().0.insert(FOE, foe);
    let mut spells = crate::ui_action::Spells::empty_for_tests();
    spells.catalog = benilla_formats::SpellCatalog::from_displays(
        [(
            DUEL,
            benilla_formats::SpellDisplay {
                effects: [83, 0, 0],
                attributes_ex: 0x0008_0000,
                targets: 0x2,
                range_index: 5,
                ..Default::default()
            },
        )]
        .into(),
    );
    spells.ranges = benilla_formats::SpellRangeCatalog::from_rows(
        [(
            5,
            benilla_formats::SpellRange {
                min: 0.0,
                max: 10.0,
                flags: 0,
            },
        )]
        .into(),
    );
    world.insert_resource(spells);
    world
        .resource_mut::<crate::ui_action::PlayerActions>()
        .spells
        .insert(DUEL);
    vm(&mut app).set_unit_guids(&UnitGuids {
        player: ME,
        party: [FOE, 0, 0, 0],
        ..Default::default()
    });
    // The party slot `party1` is read from at the call, as the list left it.
    app.world_mut()
        .resource_mut::<crate::ui_party::GroupState>()
        .apply_list(
            0,
            0,
            vec![benilla_protocol::messages::GroupMemberEntry {
                name: "Foe".into(),
                guid: FOE,
                status: 1,
                flags: 0,
            }],
            ME,
            None,
            Some(ME),
        );
    eval::<()>(&mut app, "StartDuelUnit('party1') RandomRoll(1, 2)");
    assert_eq!(sent(&rx), ["cast 7266", "roll 1"]);
}

/// The vote `0x61bdf0` inside `RollOnLoot`'s and `ConfirmLootRoll`'s calls: `CMSG_LOOT_ROLL`
/// first (`0x61bf18`), then `CANCEL_LOOT_ROLL` (`0x61bf2a`), whose listeners run before the
/// chunk's next line; the roll is gone from the engine's list before the call returns
/// (`0x61bf32`).
#[test]
fn a_loot_vote_sends_ahead_of_its_cancel_inside_its_call() {
    for (verb, bind_on_pickup) in [("RollOnLoot", false), ("ConfirmLootRoll", true)] {
        let (mut app, rx) = client();
        app.world_mut()
            .resource_mut::<crate::ui_loot_roll::LootRolls>()
            .start(LootStartRoll {
                looted_target: 0x77,
                item_slot: 2,
                item_id: 1,
                random_property_id: 0,
                suffix_factor: 0,
                countdown_ms: 60_000,
            });
        vm(&mut app).set_loot_rolls(LootRollsState {
            rolls: vec![LootRollEntry {
                roll_id: 1,
                bind_on_pickup,
                ..Default::default()
            }],
        });
        listen(&mut app, &["CANCEL_LOOT_ROLL"], "RandomRoll(9, 9)");
        eval::<()>(&mut app, &format!("{verb}(1, 2) RandomRoll(1, 2)"));
        assert_eq!(sent(&rx), ["loot roll 2", "roll 9", "roll 1"], "{verb}");
        assert!(
            !app.world()
                .resource::<crate::ui_loot_roll::LootRolls>()
                .holds(1),
            "{verb}: the roll closed inside the call"
        );
    }
}

/// The one-packet verbs send inside their calls, in the chunk's order: `ConfirmBinder()`
/// (`0x48dd00` → `0x5dfdc0(0)`), `ConfirmSummon()` (`0x48b7d0`), `CancelMeetingStoneRequest()`
/// outside a group (`0x4ca120`), `ResetInstances()` (`0x48a6e6`) and `TogglePVP()` (`0x48d736`).
#[test]
fn the_one_packet_verbs_send_inside_their_calls() {
    const INNKEEPER: u64 = 0xF130_0000_0000_0061;
    const WARLOCK: u64 = 0x44;
    let (mut app, rx) = client();
    seat(&mut app, &[(HEALTH, 100)]);
    unit_at(&mut app, INNKEEPER, 3.0);
    let world = app.world_mut();
    world
        .resource_mut::<crate::ui_binder::BinderState>()
        .ask(INNKEEPER);
    crate::ui_summon::net::request(
        WARLOCK,
        12,
        120_000,
        false,
        0.0,
        &mut world.resource_mut::<crate::ui_summon::SummonState>(),
    );
    eval::<()>(
        &mut app,
        "RandomRoll(1, 2) ConfirmBinder() RandomRoll(2, 3) ConfirmSummon() \
         CancelMeetingStoneRequest() ResetInstances() RandomRoll(3, 4) TogglePVP() \
         RandomRoll(4, 5)",
    );
    assert_eq!(
        sent(&rx),
        [
            "roll 1",
            "bind 0xf130000000000061",
            "roll 2",
            "summon 0x44",
            "stone leave",
            "reset",
            "roll 3",
            "pvp",
            "roll 4",
        ]
    );
}

/// `CancelMeetingStoneRequest()` in a group we do not lead shows `ERR_MEETING_STONE_NOT_LEADER`
/// inside its call and sends nothing (`0x4ca14e` → `0x496720`).
#[test]
fn cancel_meeting_stone_refuses_a_follower_inside_its_call() {
    let (mut app, rx) = client();
    seat(&mut app, &[(HEALTH, 100)]);
    {
        let mut group = app
            .world_mut()
            .resource_mut::<crate::ui_party::GroupState>();
        group.in_group = true;
        group.leader = 0x22;
    }
    eval::<()>(
        &mut app,
        "ERR_MEETING_STONE_NOT_LEADER = 'You are not the party leader'",
    );
    listen(
        &mut app,
        &["UI_ERROR_MESSAGE", "UI_INFO_MESSAGE", "CHAT_MSG_SYSTEM"],
        "RandomRoll(9, 9)",
    );
    eval::<()>(&mut app, "CancelMeetingStoneRequest() RandomRoll(1, 2)");
    assert_eq!(sent(&rx), ["roll 9", "roll 1"]);
}
