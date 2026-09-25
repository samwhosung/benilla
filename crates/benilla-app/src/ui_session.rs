//! The NPC-session range guard: a window bound to a live NPC (merchant, gossip, questgiver and the
//! rest) closes client-side, sending nothing as 1.12 does, when the player walks out of range or
//! the NPC despawns; the window's feed then fires its `*_CLOSED`.
//!
//! The reference's watchdog is `0x493230`, run every frame from `CGWorldFrame`'s layer update. The
//! NPC range is the cursor's own 50/9 yd service gate ([`SERVICE_RANGE_SQ`], 5.5556 yd centre to
//! centre), and the leash belongs to the latched object, 14 yd for a player ([`leash_sq`]).

use bevy::prelude::*;

use crate::net::{ClientCommand, GuidIndex, NetCommands, SelfPlayer};
use crate::target::SERVICE_RANGE_SQ;
use benilla_world::schedule::WorldStage;

/// Owns [`InteractNpc`] and its feed, read by the `"npc"` portrait and the interaction face-me.
pub(crate) struct UiSessionPlugin;

impl Plugin for UiSessionPlugin {
    fn build(&self, app: &mut App) {
        // Inside the net chain, after the apply pass that opens and swaps sessions and before the
        // facing chain: a `MERCHANT_SHOW` handler reads `UnitName("NPC")` the frame it opens.
        app.init_resource::<InteractNpc>().add_systems(
            Update,
            feed_interact_npc
                .in_set(WorldStage::Net)
                .after(crate::net::apply_net_updates)
                .before(crate::net::drive_display_facing),
        );
    }
}

/// A UI session bound to a live NPC, or a GameObject for the mailbox: what the range guard closes.
/// The binder and talent-wipe questions are sessions too, as the reference re-runs its
/// interact-range test against a latched guid. Each NPC window registers
/// [`close_npc_session_out_of_range::<T>`] ahead of its feed so the close fires its `*_CLOSED` the
/// same frame; trade does not, as its cancel is server-driven.
pub(crate) trait NpcSession: Resource {
    /// The bound NPC's guid; `None` when no window is open.
    fn npc(&self) -> Option<u64>;
    /// The window's client-side close, with no packet: the same clear its close button does.
    fn close(&mut self);
    /// What a walk-away close sends first, if anything. The reference's watchdog (`0x4933da`)
    /// calls the quest session's end (`0x501130`) directly, not `DeclineQuest`, so walking away
    /// from an NPC's quest panel sends nothing, while a shared quest still owes the sharer
    /// `MSG_QUEST_PUSH_RESULT{DECLINE_QUEST}`.
    fn walk_away_send(&self, _npc: u64) -> Option<ClientCommand> {
        None
    }
}

/// The walk-away leash, set by the latched object's type, not the window: the reference's
/// `0x4930d0` stores the opener's range and overwrites it for an object with the PLAYER typemask
/// bit (`0x493156`). Thirteen of the client's fourteen arming sites push 50/9 yd
/// ([`SERVICE_RANGE_SQ`]); a player gets `10.0 + 4.0 = 14.0` yd (`[0x8044b0] + [0x80306c]`,
/// squared at `0x49316d`), which here means a shared quest's sharer.
fn leash_sq(npc: u64) -> f32 {
    if benilla_protocol::guid::is_player(npc) {
        PLAYER_LEASH_SQ
    } else {
        SERVICE_RANGE_SQ
    }
}

/// `14.0²`, the leash for a session latched onto a player.
const PLAYER_LEASH_SQ: f32 = 196.0;

/// Whether an open NPC window switched to a different NPC (`Some(a)` to `Some(b)`). Stock
/// `ShowUIPanel` returns early for a visible frame (`UIParent.lua:650`), so the client fires
/// `*_CLOSED` then `*_SHOW`, both sounds; each feed does the same, then consumes the close intent
/// its own `*_CLOSED` queued so the drain does not wipe the new session.
pub(crate) fn npc_switched(prev: Option<u64>, now: Option<u64>) -> bool {
    matches!((prev, now), (Some(a), Some(b)) if a != b)
}

/// Close `T`'s open session when the player leaves the leash or the NPC despawns.
pub(crate) fn close_npc_session_out_of_range<T: NpcSession>(
    mut session: ResMut<T>,
    index: Res<GuidIndex>,
    commands: Res<NetCommands>,
    self_q: Query<&Transform, With<SelfPlayer>>,
    transforms: Query<&Transform>,
) {
    let Some(npc) = session.npc() else { return };
    let Some(self_tf) = self_q.iter().next() else {
        return;
    };
    // Centre to centre with no bounding radii, and `dist² <= leash²` keeps: the reference's
    // compare (`0x4932f2`, `0x4932fe`).
    let out = match index.0.get(&npc).and_then(|e| transforms.get(*e).ok()) {
        Some(tf) => tf.translation.distance_squared(self_tf.translation) > leash_sq(npc),
        None => true, // the NPC despawned out from under the window
    };
    if out {
        debug!("ui_session: NPC {npc:#x} out of range/gone — client-side close");
        if let Some(cmd) = session.walk_away_send(npc) {
            let _ = commands.0.send(cmd);
        }
        session.close();
    }
}

/// The `"npc"` unit: the open [`NpcSession`]'s world entity and guid, read by the portrait booth
/// as it reads [`crate::target::Selection`] for `"target"`. Only one session is open in play.
#[derive(Resource, Default, PartialEq, Eq)]
pub(crate) struct InteractNpc(pub(crate) Option<Entity>, pub(crate) Option<u64>);

/// Collapse the portrait-bound sessions into [`InteractNpc`]: they are mutually exclusive, so the
/// first open one in the chain wins. `Option<Res<_>>` lets a test mount it without every window.
pub(crate) fn feed_interact_npc(
    gossip: Option<Res<crate::ui_gossip::GossipState>>,
    quest: Option<Res<crate::ui_quest::QuestGiver>>,
    merchant: Option<Res<crate::ui_merchant::MerchantOpen>>,
    trainer: Option<Res<crate::ui_trainer::TrainerOpen>>,
    taxi: Option<Res<crate::ui_taxi::TaxiState>>,
    // Trade's "npc" is the partner, a live player.
    trade: Option<Res<crate::ui_trade::TradeSession>>,
    bank: Option<Res<crate::ui_bank::BankOpen>>,
    auction: Option<Res<crate::ui_auction::AuctionOpen>>,
    // Not covered by the gossip arm: vmangos closes the gossip menu before it sends
    // `SMSG_PETITION_SHOWLIST` (`Player.cpp:12428-12431`).
    registrar: Option<Res<crate::ui_petition::GuildRegistrarState>>,
    // A menuless stable master is asked for its list straight from the interact, the reference's
    // `0x5f05bc` path, with no gossip session behind it.
    stable: Option<Res<crate::ui_stable::StableOpen>>,
    tabard: Option<Res<crate::ui_tabard::TabardOpen>>,
    index: Option<Res<GuidIndex>>,
    mut out: ResMut<InteractNpc>,
) {
    let guid = gossip
        .and_then(|s| s.npc())
        .or_else(|| quest.and_then(|s| s.npc()))
        .or_else(|| merchant.and_then(|s| s.npc()))
        .or_else(|| trainer.and_then(|s| s.npc()))
        .or_else(|| taxi.and_then(|s| s.npc()))
        .or_else(|| trade.and_then(|s| s.npc()))
        .or_else(|| bank.and_then(|s| s.npc()))
        .or_else(|| auction.and_then(|s| s.npc()))
        .or_else(|| registrar.and_then(|s| s.npc()))
        .or_else(|| stable.and_then(|s| s.npc()))
        .or_else(|| tabard.and_then(|s| s.npc()));
    // Field 0 is the entity the booth bakes and the facing chain steers by; field 1 is the guid
    // `crate::ui_unit` names the token by, which holds even when the entity is not streamed.
    // Written only on a change: the unit feed's dirty gate reads `is_changed()` on it.
    let next = InteractNpc(
        guid.and_then(|g| index.and_then(|i| i.0.get(&g).copied())),
        guid,
    );
    out.set_if_neq(next);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_merchant::MerchantOpen;

    /// A `HIGHGUID_UNIT` guid: a bare `0x42` would be a player's (`guid::is_player`) and get the
    /// 14 yd leash.
    const VENDOR: u64 = 0xF130_0000_0000_0042;

    #[test]
    fn out_of_range_or_despawned_npc_closes_the_session() {
        let mut app = App::new();
        app.init_resource::<MerchantOpen>();
        app.init_resource::<GuidIndex>();
        // The merchant owes no walk-away send, but the system needs the channel.
        let (tx, _rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        app.add_systems(Update, close_npc_session_out_of_range::<MerchantOpen>);
        app.world_mut()
            .spawn((SelfPlayer, Transform::from_xyz(0.0, 0.0, 0.0)));
        let vendor = app
            .world_mut()
            .spawn(Transform::from_xyz(5.0, 0.0, 0.0))
            .id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(VENDOR, vendor);

        // 5.0 yd < the 5.5556 yd service gate: stays open.
        app.world_mut()
            .resource_mut::<MerchantOpen>()
            .open(VENDOR, vec![]);
        app.update();
        assert!(app.world().resource::<MerchantOpen>().is_open());

        // 6 yd: past the gate, so it closes.
        *app.world_mut()
            .entity_mut(vendor)
            .get_mut::<Transform>()
            .unwrap() = Transform::from_xyz(6.0, 0.0, 0.0);
        app.update();
        assert!(!app.world().resource::<MerchantOpen>().is_open());

        // Re-open, then the NPC despawns out from under the window: closes too.
        app.world_mut()
            .resource_mut::<MerchantOpen>()
            .open(VENDOR, vec![]);
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .remove(&VENDOR);
        app.update();
        assert!(!app.world().resource::<MerchantOpen>().is_open());
    }

    /// A player-latched quest panel is leashed at 14 yd and an NPC-latched one at 5.56 yd; walking
    /// away from a share still declines it, from an NPC's panel it sends nothing.
    #[test]
    fn the_walk_away_leash_is_the_givers_own_and_a_share_still_answers() {
        use crate::ui_quest::{QuestGiver, QuestView};

        fn detail(npc: u64) -> QuestView {
            QuestView::Detail(benilla_protocol::messages::QuestDetails {
                npc,
                quest_id: 1,
                title: "A Threat Within".into(),
                details: String::new(),
                objectives: String::new(),
                auto_finish: 0,
                choices: Vec::new(),
                rewards: Vec::new(),
                money: 0,
                reward_spell: 0,
            })
        }

        const SHARER: u64 = 0x0000_0000_0000_002A; // HIGHGUID_PLAYER: a zero high word
        const NPC: u64 = 0xF130_0000_0000_0007; // HIGHGUID_UNIT

        /// One guard pass with the panel on `giver` at `dist` yd: whether it stays, what is sent.
        fn walk(giver: u64, dist: f32) -> (bool, Vec<ClientCommand>) {
            let (tx, rx) = crossbeam_channel::unbounded();
            let mut app = App::new();
            app.insert_resource(NetCommands(tx));
            app.init_resource::<QuestGiver>();
            app.init_resource::<GuidIndex>();
            app.add_systems(Update, close_npc_session_out_of_range::<QuestGiver>);
            app.world_mut()
                .spawn((SelfPlayer, Transform::from_xyz(0.0, 0.0, 0.0)));
            let e = app
                .world_mut()
                .spawn(Transform::from_xyz(dist, 0.0, 0.0))
                .id();
            app.world_mut()
                .resource_mut::<GuidIndex>()
                .0
                .insert(giver, e);
            app.world_mut()
                .resource_mut::<QuestGiver>()
                .open(giver, detail(giver));
            app.update();
            (
                app.world().resource::<QuestGiver>().is_open(),
                rx.try_iter().collect(),
            )
        }

        // A share at 10 yd: past the NPC gate, inside the player leash.
        let (open, sent) = walk(SHARER, 10.0);
        assert!(open, "a share is leashed at 14 yd, not 5.56");
        assert!(sent.is_empty(), "still in range, nothing owed: {sent:?}");

        // Past 14 yd it closes and answers the sharer.
        let (open, sent) = walk(SHARER, 15.0);
        assert!(!open, "past the player leash the panel closes");
        assert!(
            matches!(
                sent.as_slice(),
                [ClientCommand::QuestPushResult {
                    sharer: SHARER,
                    msg: benilla_protocol::messages::QuestShareMsg::DECLINE_QUEST,
                }]
            ),
            "walking away from a share still declines it: {sent:?}"
        );

        // An NPC giver at 10 yd: closed, silently.
        let (open, sent) = walk(NPC, 10.0);
        assert!(!open, "an NPC giver is still leashed at 5.56 yd");
        assert!(
            sent.is_empty(),
            "the walk-away skips the giver re-open the button does: {sent:?}"
        );

        // Boundary-inclusive: exactly on the leash keeps the window (`dist² <= leash²`).
        assert!(walk(SHARER, 14.0).0, "14.0 yd exactly is still in range");
    }

    /// No window is `None`, an unstreamed NPC is `None` and never a stale entity, and gossip wins
    /// the chain when two sessions hold a guid (the same NPC on a browse-goods hop).
    #[test]
    fn interact_npc_resolves_the_open_session_to_its_entity() {
        use crate::ui_gossip::GossipState;

        let mut app = App::new();
        app.init_resource::<GossipState>();
        app.init_resource::<MerchantOpen>();
        app.init_resource::<GuidIndex>();
        app.init_resource::<InteractNpc>();
        app.add_systems(Update, feed_interact_npc);

        let npc = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(VENDOR, npc);

        app.update();
        assert_eq!(app.world().resource::<InteractNpc>().0, None);

        app.world_mut()
            .resource_mut::<MerchantOpen>()
            .open(VENDOR, vec![]);
        app.update();
        assert_eq!(app.world().resource::<InteractNpc>().0, Some(npc));

        // A browse-goods hop: gossip holds the same guid and wins the chain.
        app.world_mut().resource_mut::<GossipState>().npc = Some(VENDOR);
        app.update();
        assert_eq!(app.world().resource::<InteractNpc>().0, Some(npc));

        // A guid with no index entry, an NPC not streamed.
        app.world_mut().resource_mut::<GossipState>().npc = Some(0xdead);
        app.update();
        assert_eq!(app.world().resource::<InteractNpc>().0, None);

        // A session missing from the chain renders a black disc where the NPC's face goes.
        app.world_mut().resource_mut::<GossipState>().npc = None;
        app.world_mut().resource_mut::<MerchantOpen>().close();
        app.init_resource::<crate::ui_auction::AuctionOpen>();
        app.world_mut()
            .resource_mut::<crate::ui_auction::AuctionOpen>()
            .open(VENDOR, 1);
        app.update();
        assert_eq!(
            app.world().resource::<InteractNpc>().0,
            Some(npc),
            "the auctioneer's own portrait"
        );

        app.world_mut()
            .resource_mut::<crate::ui_auction::AuctionOpen>()
            .clear();

        // The stable master: no gossip session carries it.
        app.init_resource::<crate::ui_stable::StableOpen>();
        app.world_mut()
            .resource_mut::<crate::ui_stable::StableOpen>()
            .open(VENDOR, 2, vec![]);
        app.update();
        assert_eq!(
            app.world().resource::<InteractNpc>().0,
            Some(npc),
            "the stable master's own portrait"
        );

        app.world_mut().resource_mut::<GossipState>().close();
        app.world_mut().resource_mut::<MerchantOpen>().close();
        app.world_mut()
            .resource_mut::<crate::ui_stable::StableOpen>()
            .clear();
        app.update();
        assert_eq!(app.world().resource::<InteractNpc>().0, None);
    }

    /// Every `NpcSession` implementor in the app's sources must be in [`feed_interact_npc`]'s
    /// chain or on the exclusion list with a reason; one missing renders a black portrait disc.
    #[test]
    fn every_npc_session_is_portrait_bound_or_explicitly_excluded() {
        use std::path::Path;

        /// Sessions that do not own the `"npc"` portrait token, each with its reason.
        const EXCLUDED: &[(&str, &str)] = &[
            // Its icon is art, not a unit bake, and must not take a portrait window's token.
            ("MailOpen", "its window icon is art, not a unit bake (0544)"),
            // Reached from an open gossip menu, which stays open behind it and heads the chain;
            // unlike the registrar's, the server does not close it first.
            ("TalentWipeState", "rides the still-open gossip session"),
            ("BinderState", "rides the still-open gossip session"),
            // The pet trainer's question, reached the same way.
            ("PetUnlearnState", "rides the still-open gossip session"),
        ];

        // The chain's own source says what is wired.
        let this_file = include_str!("ui_session.rs");
        let chain = this_file
            .split_once("pub(crate) fn feed_interact_npc")
            .expect("feed_interact_npc")
            .1
            .split_once("\n}\n")
            .expect("end of feed_interact_npc")
            .0;

        let mut missing = Vec::new();
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("read_dir") {
                let path = entry.expect("dir entry").path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") {
                    continue;
                }
                // This file defines the trait and no session, but its own text spells the pattern.
                if path.file_name().is_some_and(|f| f == "ui_session.rs") {
                    continue;
                }
                let src = std::fs::read_to_string(&path).expect("read source");
                for (_, rest) in src
                    .match_indices("impl NpcSession for ")
                    .map(|(i, _)| (i, &src[i + "impl NpcSession for ".len()..]))
                {
                    let ty: String = rest
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect();
                    if ty.is_empty() || EXCLUDED.iter().any(|(name, _)| *name == ty) {
                        continue;
                    }
                    // Wired means the chain's text names the type, as its `Res<...>` parameter
                    // does; a comment naming it counts too.
                    if !chain.contains(&ty) {
                        missing.push(format!("{ty} (in {})", path.display()));
                    }
                }
            }
        }
        assert!(
            missing.is_empty(),
            "these NpcSession windows own no `\"npc\"` portrait and are not on the exclusion \
             list, so each renders a BLACK DISC where the NPC's face goes — wire them into \
             `feed_interact_npc`'s chain, or add them to EXCLUDED with a reason: {missing:#?}"
        );
    }

    /// Checked in the built schedule graph, by system identity: a `.after()` on a system missing
    /// from the schedule is silently nothing, and debug names may be off.
    #[test]
    fn the_interact_npc_writer_is_seated_between_the_net_apply_and_the_facing_chain() {
        use bevy::ecs::schedule::graph::Direction::{Incoming, Outgoing};
        use bevy::ecs::schedule::{NodeId, Schedule};
        use bevy::ecs::system::{IntoSystem, System};
        use std::any::TypeId;
        use std::collections::HashSet;

        /// `a` runs before `b`: the dependency graph walked with the set hierarchy unfolded, so a
        /// set `a` sits in precedes what that set precedes, and a successor set brings its members.
        fn runs_before(schedule: &Schedule, a: NodeId, b: NodeId) -> bool {
            let dep = schedule.graph().dependency().graph();
            let hier = schedule.graph().hierarchy().graph();
            let (mut after, mut containers) = (HashSet::new(), HashSet::new());
            let mut work = vec![(a, false)];
            while let Some((n, is_after)) = work.pop() {
                let fresh = if is_after {
                    after.insert(n)
                } else {
                    containers.insert(n)
                };
                if !fresh {
                    continue;
                }
                work.extend(dep.neighbors_directed(n, Outgoing).map(|m| (m, true)));
                work.extend(hier.neighbors_directed(n, Incoming).map(|p| (p, false)));
                if is_after {
                    work.extend(hier.neighbors_directed(n, Outgoing).map(|c| (c, true)));
                }
            }
            after.contains(&b)
        }

        let apply = System::type_id(&IntoSystem::into_system(crate::net::apply_net_updates));
        let writer = System::type_id(&IntoSystem::into_system(feed_interact_npc));
        let facing = System::type_id(&IntoSystem::into_system(crate::net::drive_display_facing));

        let mut app = App::new();
        app.add_plugins((crate::net::NetPlugin { connect: false }, UiSessionPlugin));
        // `schedule_scope`, not `resource_scope::<Schedules>`: initializing the schedule inserts
        // `Schedules`, which a resource scope refuses.
        app.world_mut().schedule_scope(Update, |world, schedule| {
            schedule
                .initialize(world)
                .expect("the Update schedule builds");
            let key = |id: TypeId| -> NodeId {
                schedule
                    .systems()
                    .expect("initialized")
                    .find(|&(_, s)| System::type_id(&**s) == id)
                    .map(|(k, _)| NodeId::System(k))
                    .expect("the system is in Update")
            };
            let (apply, writer, facing) = (key(apply), key(writer), key(facing));
            assert!(
                runs_before(schedule, apply, writer),
                "feed_interact_npc must run after apply_net_updates"
            );
            assert!(
                runs_before(schedule, writer, facing),
                "drive_display_facing must run after feed_interact_npc"
            );
        });
    }
}
