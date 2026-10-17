//! The NPC interaction: the latch every NPC window registers on and closes through ([`latch`]),
//! the `"npc"` unit it publishes ([`InteractNpc`]), and the range guard of the questions that keep
//! a guid of their own ([`NpcSession`]).
//!
//! The reference's watchdog is `0x493230`, run every frame from `CGWorldFrame`'s layer update. The
//! NPC range is the cursor's own 50/9 yd service gate ([`SERVICE_RANGE_SQ`], 5.5556 yd centre to
//! centre), and the leash belongs to the latched object, 14 yd for a player ([`leash_sq`]).

use bevy::prelude::*;

use crate::net::{GuidIndex, NetHandlerApp, SelfPlayer};
use crate::target::SERVICE_RANGE_SQ;
use crate::ui_script::SweepApp;
use benilla_protocol::SessionEventKind;
use benilla_world::schedule::WorldStage;

mod latch;
#[cfg(test)]
mod latch_tests;
mod release;
#[cfg(test)]
mod release_tests;

pub(crate) use latch::{
    dispatch, fire, note_closed, queue, register, release, with_script, CoreCloses, InteractLatch,
    LatchApp, LatchVocal, LatchedWindow, NpcWindow,
};

/// Owns [`InteractLatch`], its watchdog and [`InteractNpc`], read by the `"npc"` portrait and the
/// interaction face-me.
pub(crate) struct UiSessionPlugin;

impl Plugin for UiSessionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<InteractLatch>()
            .add_message::<LatchVocal>()
            .init_resource::<InteractNpc>()
            .net_handler(
                SessionEventKind::Disconnected,
                latch::clear_latch_on_session_end,
            )
            // `0x490b5f` → `0x493310`: whatever window still holds the latch past
            // `PLAYER_LEAVING_WORLD` closes through the dispatcher.
            .add_sweep_leg(crate::ui_script::SweepLeg::Interaction, dispatch)
            .add_systems(
                Update,
                (
                    // Before the feeds, so a walk-away's `*_CLOSED` lands the frame it happens.
                    latch::watch_interaction_range.before(crate::ui_script::UiFeed),
                    // The unit events that release the latch, off the apply pass's field edges.
                    release::release_on_field_edges
                        .in_set(WorldStage::Net)
                        .after(crate::net::apply_net_updates)
                        .before(feed_interact_npc),
                    // Inside the net chain, after the apply pass that opens and closes windows
                    // and before the facing chain: a `MERCHANT_SHOW` handler reads
                    // `UnitName("NPC")` the frame it opens.
                    feed_interact_npc
                        .in_set(WorldStage::Net)
                        .after(crate::net::apply_net_updates)
                        .before(crate::net::drive_display_facing),
                ),
            );
    }
}

/// A question bound to a live NPC with a guid of its own, not the latch: the binder and
/// talent-wipe questions and the pet trainer's, as the reference re-runs its interact-range test
/// against each one's latched guid. Each registers [`close_npc_session_out_of_range::<T>`] ahead
/// of its feed.
pub(crate) trait NpcSession: Resource {
    /// The bound NPC's guid; `None` when nothing is open.
    fn npc(&self) -> Option<u64>;
    /// The client-side close, with no packet.
    fn close(&mut self);
}

/// The walk-away leash, set by the latched object's type, not the window: the reference's
/// `0x4930d0` stores the opener's range and overwrites it for an object with the PLAYER typemask
/// bit (`0x493156`). Thirteen of the client's fourteen arming sites push 50/9 yd
/// ([`SERVICE_RANGE_SQ`]); a player gets `10.0 + 4.0 = 14.0` yd (`[0x8044b0] + [0x80306c]`,
/// squared at `0x49316d`), which here means a shared quest's sharer or a trade partner.
fn leash_sq(npc: u64) -> f32 {
    if benilla_protocol::guid::is_player(npc) {
        PLAYER_LEASH_SQ
    } else {
        SERVICE_RANGE_SQ
    }
}

/// `14.0²`, the leash for a session latched onto a player.
const PLAYER_LEASH_SQ: f32 = 196.0;

/// Close `T`'s open question when the player leaves the leash or the NPC despawns.
pub(crate) fn close_npc_session_out_of_range<T: NpcSession>(
    mut session: ResMut<T>,
    index: Res<GuidIndex>,
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
        None => true, // the NPC despawned out from under the question
    };
    if out {
        debug!("ui_session: NPC {npc:#x} out of range/gone — client-side close");
        session.close();
    }
}

/// The `"npc"` unit: the latched object's world entity and guid, read by the portrait booth as it
/// reads [`crate::target::Selection`] for `"target"`.
#[derive(Resource, Default, PartialEq, Eq, Debug)]
pub(crate) struct InteractNpc(pub(crate) Option<Entity>, pub(crate) Option<u64>);

/// Publish the latch as [`InteractNpc`]. The reference's `"npc"` is the latch itself (`0x515c00`),
/// a trade's partner included, but a quest panel opened from a bag item, the mailbox and the text
/// reader publish none here, as before the latch: there is no unit bake for an item or a game
/// object window. `Option<Res<_>>` lets a test mount it without every window.
pub(crate) fn feed_interact_npc(
    latch: Option<Res<InteractLatch>>,
    mail: Option<Res<crate::ui_mail::MailOpen>>,
    reader: Option<Res<crate::ui_item_text::ItemTextOpen>>,
    index: Option<Res<GuidIndex>>,
    mut out: ResMut<InteractNpc>,
) {
    let guid = latch.and_then(|l| l.guid()).filter(|&g| {
        !benilla_protocol::guid::is_item(g)
            && mail.as_ref().is_none_or(|m| m.mailbox != Some(g))
            && reader.as_ref().is_none_or(|r| r.object() != Some(g))
    });
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
    use crate::net::NetCommands;
    use crate::ui_merchant::MerchantOpen;

    /// A `HIGHGUID_UNIT` guid: a bare `0x42` would be a player's (`guid::is_player`) and get the
    /// 14 yd leash.
    const VENDOR: u64 = 0xF130_0000_0000_0042;

    /// An app with `T`'s window on the latch, its watchdog, the player at the origin and `guid`'s
    /// object `dist` yd down the x axis.
    fn latched_app<T: LatchedWindow + Default>(guid: u64, dist: f32) -> (App, Entity) {
        let mut app = App::new();
        app.init_resource::<T>()
            .init_resource::<GuidIndex>()
            .add_latched_window::<T>()
            .add_systems(Update, latch::watch_interaction_range);
        app.world_mut()
            .spawn((SelfPlayer, Transform::from_xyz(0.0, 0.0, 0.0)));
        let e = app
            .world_mut()
            .spawn(Transform::from_xyz(dist, 0.0, 0.0))
            .id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(guid, e);
        (app, e)
    }

    /// The watchdog `0x493230` closes the latched window when its NPC leaves the 50/9 yd service
    /// gate or despawns, and leaves it inside the gate (`dist² <= leash²`).
    #[test]
    fn out_of_range_or_despawned_npc_closes_the_latched_window() {
        let (mut app, vendor) = latched_app::<MerchantOpen>(VENDOR, 5.0);
        crate::ui_merchant::open_merchant(app.world_mut(), VENDOR, vec![]);

        // 5.0 yd < the 5.5556 yd service gate: stays open.
        app.update();
        assert!(app.world().resource::<MerchantOpen>().is_open());
        assert_eq!(app.world().resource::<InteractLatch>().guid(), Some(VENDOR));

        // 6 yd: past the gate, so it closes, and the latch with it.
        *app.world_mut()
            .entity_mut(vendor)
            .get_mut::<Transform>()
            .unwrap() = Transform::from_xyz(6.0, 0.0, 0.0);
        app.update();
        assert!(!app.world().resource::<MerchantOpen>().is_open());
        assert_eq!(app.world().resource::<InteractLatch>().guid(), None);

        // Re-open, then the NPC despawns out from under the window: closes too.
        crate::ui_merchant::open_merchant(app.world_mut(), VENDOR, vec![]);
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .remove(&VENDOR);
        app.update();
        assert!(!app.world().resource::<MerchantOpen>().is_open());
    }

    /// A player-latched quest panel is leashed at 14 yd and an NPC-latched one at 5.56 yd; walking
    /// away from a share still declines it (`0x4933d3` → `0x501130(0, 1)`), from an NPC's panel
    /// it sends nothing.
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

        /// One watchdog pass with the panel on `giver` at `dist` yd: whether it stays, what is
        /// sent.
        fn walk(giver: u64, dist: f32) -> (bool, Vec<crate::net::ClientCommand>) {
            let (tx, rx) = crossbeam_channel::unbounded();
            let (mut app, _) = latched_app::<QuestGiver>(giver, dist);
            app.insert_resource(NetCommands(tx));
            crate::ui_quest::open_panel(app.world_mut(), giver, detail(giver));
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
                [crate::net::ClientCommand::QuestPushResult {
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

        // A quest-starter item is never leashed (`0x4932b5`): its panel stays however far.
        const ITEM: u64 = 0x4000_0000_0000_0BAD; // HIGHGUID_ITEM
        assert!(
            walk(ITEM, 100.0).0,
            "an item's panel is never walked away from"
        );
    }

    /// No latch is `None`, an unstreamed NPC is `None` and never a stale entity, an item's quest
    /// panel publishes none, and a window off the latch still publishes its own.
    #[test]
    fn interact_npc_publishes_the_latch_as_its_entity() {
        let mut app = App::new();
        app.init_resource::<InteractLatch>();
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
            .resource_mut::<InteractLatch>()
            .latch_for_test(VENDOR);
        app.update();
        assert_eq!(
            *app.world().resource::<InteractNpc>(),
            InteractNpc(Some(npc), Some(VENDOR))
        );

        // A guid with no index entry, an NPC not streamed: the guid names the token, no entity.
        app.world_mut()
            .resource_mut::<InteractLatch>()
            .latch_for_test(0xF130_0000_0000_DEAD);
        app.update();
        assert_eq!(app.world().resource::<InteractNpc>().0, None);

        // A quest-starter item holds the latch: no unit to name.
        const ITEM: u64 = 0x4000_0000_0000_0BAD;
        app.world_mut()
            .resource_mut::<InteractLatch>()
            .latch_for_test(ITEM);
        app.update();
        assert_eq!(
            *app.world().resource::<InteractNpc>(),
            InteractNpc(None, None)
        );

        // The mailbox holds the latch: no unit to name.
        const MAILBOX: u64 = 0xF110_0000_0000_0B0C;
        app.init_resource::<crate::ui_mail::MailOpen>();
        app.world_mut()
            .resource_mut::<crate::ui_mail::MailOpen>()
            .click(MAILBOX);
        app.world_mut()
            .resource_mut::<InteractLatch>()
            .latch_for_test(MAILBOX);
        app.update();
        assert_eq!(
            *app.world().resource::<InteractNpc>(),
            InteractNpc(None, None)
        );

        // A trade's partner holds the latch: the partner's own portrait.
        const PARTNER: u64 = 0x0000_0000_0000_0008;
        let partner = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(PARTNER, partner);
        app.world_mut()
            .resource_mut::<InteractLatch>()
            .latch_for_test(PARTNER);
        app.update();
        assert_eq!(
            app.world().resource::<InteractNpc>().0,
            Some(partner),
            "the trade partner's own portrait"
        );
    }

    /// Every `NpcSession` implementor in the app's sources must be in [`feed_interact_npc`]'s
    /// chain or on the exclusion list with a reason; one missing renders a black portrait disc.
    #[test]
    fn every_npc_session_is_portrait_bound_or_explicitly_excluded() {
        use std::path::Path;

        /// Sessions that do not own the `"npc"` portrait token, each with its reason.
        const EXCLUDED: &[(&str, &str)] = &[
            // Reached from an open gossip menu, which stays open behind it holding the latch;
            // unlike the registrar's, the server does not close it first.
            ("TalentWipeState", "rides the still-open gossip session"),
            ("BinderState", "rides the still-open gossip session"),
            // The pet trainer's question, reached the same way.
            ("PetUnlearnState", "rides the still-open gossip session"),
        ];

        // The chain's own source says what is wired.
        // Line endings normalised first: the end of the function is found on `\n`.
        let this_file = include_str!("ui_session.rs").replace("\r\n", "\n");
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

    #[test]
    fn the_interact_npc_writer_is_seated_between_the_net_apply_and_the_facing_chain() {
        use crate::test_support::runs_before;
        let mut app = App::new();
        app.add_plugins((crate::net::NetPlugin { connect: false }, UiSessionPlugin));
        assert!(
            runs_before(&mut app, crate::net::apply_net_updates, feed_interact_npc),
            "feed_interact_npc must run after apply_net_updates"
        );
        assert!(
            runs_before(
                &mut app,
                feed_interact_npc,
                crate::net::drive_display_facing
            ),
            "drive_display_facing must run after feed_interact_npc"
        );
    }
}
