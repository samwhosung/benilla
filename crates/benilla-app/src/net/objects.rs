//! The bridge's object layer: the streamed world's creates, deltas, moves and destroys, movers'
//! speeds and modes, GameObject templates and anims, and items. Each event is one handler over
//! [`Scene`] whose commands apply before the next event's, so a create's store and speeds are live
//! components by the time the next block reads them. An `SMSG_UPDATE_OBJECT`'s blocks and its
//! passes are [`update`]'s.
use benilla_assets::coords::bevy_to_wow;
use benilla_protocol::{
    EntityKind, MonsterMoveFacing, MoveSpeeds, ObjectFields, SpeedKind, SplineMode,
};
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::go_templates::GameObjectTemplates;
use crate::items::Items;
use crate::names::NameCache;
use benilla_world::model_fade::DespawnFade;
use benilla_world::vis_chain::VisChainOnly;

use super::motion::{
    catch_up, create_spline, gameobject_rotation, monster_move_spline, pose_transform,
    resolve_facing, set_raw_facing, smoothed, trace_create_spline, trace_move_snap, wire_yaw,
    write_pose, MoverWorld, SplineStopped, ROOT_APPLY_WIPE,
};
use super::{
    ActiveMover, FieldChanged, Guid, GuidIndex, NetCommands, NetEntity, NetHandlerApp, ObjectStore,
    RelaySeed, RemoteMotion, SelfGuid, SelfPlayer, SpeedChangeMessage, Spline, UnitMoveModes,
    UnitSpeeds,
};

mod arrival;
mod item_deactivate;
mod update;

pub(crate) use update::{release_unheld, Listed, ReleasePending, Released};

/// `SMSG_GAMEOBJECT_CUSTOM_ANIM` and `SMSG_GAMEOBJECT_DESPAWN_ANIM` (`0x5f8930`, `0x5f8990`): an
/// event on the object's one-shot channel (`0x5f8c50`), armed at the packet so the hold its play
/// takes is there for a release in the same tick; vmangos sends `SMSG_DESTROY_OBJECT` right after
/// the despawn. An object whose model has no state machine yet drops the event.
fn gameobject_anim_event(
    guid: u64,
    index: &GuidIndex,
    go_anims: &mut Query<&mut crate::go_anim::GoAnim>,
    arm: impl FnOnce(&mut crate::go_anim::GoAnim),
) {
    if let Some(mut go) = index.0.get(&guid).and_then(|&e| go_anims.get_mut(e).ok()) {
        arm(&mut go);
    }
}

/// Registers the object layer's handlers.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    update::register(app);
    app.net_handler(K::ItemTemplate, on_item_answer)
        .net_handler(K::ItemTime, on_item)
        .net_handler(K::ItemEnchantTime, on_item)
        .net_handler(K::MoveTimeSkipped, on_move_time_skipped)
        .net_handler(K::UnitMove, on_unit_move)
        .net_handler(K::MonsterMove, on_monster_move)
        .net_handler(K::GameObjectInfo, on_gameobject_info)
        .net_handler(K::GameObjectUnknown, on_gameobject_unknown)
        .net_handler(K::GameObjectCustomAnim, on_gameobject_anim)
        .net_handler(K::GameObjectDespawnAnim, on_gameobject_anim)
        .net_handler(K::SplineMoveMode, on_spline_move_mode)
        .net_handler(K::ForceSpeedChange, on_speed)
        .net_handler(K::SpeedChanged, on_speed);
}

/// The streamed world as one parameter: the components, caches, lifecycle hooks and edges the
/// object packets touch.
#[derive(SystemParam)]
pub(crate) struct Scene<'w, 's> {
    commands: Commands<'w, 's>,
    net: Res<'w, NetCommands>,
    index: ResMut<'w, GuidIndex>,
    self_guid: Res<'w, SelfGuid>,
    real: Res<'w, Time<Real>>,
    /// The movement clock a relayed move is due by at its arrival (`@0x618dd2`).
    move_tick: Option<Res<'w, super::MoveTick>>,
    transforms: Query<'w, 's, &'static mut Transform>,
    movers: Query<'w, 's, (), With<ActiveMover>>,
    stores: Query<'w, 's, &'static mut ObjectStore>,
    countdowns: Query<'w, 's, &'static mut crate::items::Countdowns>,
    remote: Query<'w, 's, &'static mut RemoteMotion>,
    /// What a create left on a unit's mover before its first relay.
    seeds: Query<'w, 's, &'static mut RelaySeed>,
    modes: Query<'w, 's, &'static mut UnitMoveModes>,
    xp_awarded: Query<'w, 's, (), With<crate::ui_chat::combat::watch::XpAwarded>>,
    client_controlled: Query<'w, 's, (), With<super::ClientControlled>>,
    riders: Query<'w, 's, &'static mut crate::transport::TransportRider>,
    /// A transport's clock, which only a new object's create sets (`0x5f7080`).
    anchors: Query<'w, 's, &'static crate::transport::TransportAnchor>,
    speeds: Query<'w, 's, &'static mut UnitSpeeds>,
    field_changes: MessageWriter<'w, FieldChanged>,
    /// The display rebuilds (`0x60abe0`) the watchers run, at their places.
    rebuilds: MessageWriter<'w, crate::entities::DisplayRebuilt>,
    /// The aura watcher's calls ([`super::AuraFieldsChanged`]), one per block.
    aura_watch: MessageWriter<'w, super::AuraFieldsChanged>,
    /// The item watchers' runs ([`crate::item_lane`]).
    item_lane: ResMut<'w, crate::item_lane::ItemLane>,
    /// The bag cache `0xbdd060` the bag seats' runs rescan and the bag events number from.
    bag_cache: ResMut<'w, crate::item_watch::BagCache>,
    /// What each packet changed in the stores, for the views a listener reads
    /// ([`crate::ui_unit::views`]).
    view_log: ResMut<'w, crate::ui_unit::views::ViewLog>,
    field_notices: ResMut<'w, crate::field_watch::NoticeLane>,
    /// The guid-scoped selection teardowns (`0x493910(guid, 1)`) a watcher raises, run at the
    /// packet's end; absent in a harness without targets.
    deselect: Option<ResMut<'w, crate::target::OwedSelects>>,
    speed_changes: MessageWriter<'w, SpeedChangeMessage>,
    self_moves: MessageWriter<'w, super::SelfMoveMessage>,
    hard_landings: MessageWriter<'w, crate::creature_anim::HardLanding>,
    names: Res<'w, NameCache>,
    items: ResMut<'w, Items>,
    go_templates: ResMut<'w, GameObjectTemplates>,
    death_net: ResMut<'w, crate::death::DeathNet>,
    group: ResMut<'w, crate::ui_party::GroupState>,
    pet_bar: Option<Res<'w, crate::ui_pet::PetBar>>,
    /// The selection, whose aura state the usable refresh watches (`0x6048a6`), and which a
    /// unit's release ends and a create or a reactivation holds again ([`crate::target`]).
    selection: Option<ResMut<'w, crate::target::Selection>>,
    /// The selection teardowns the packet's releases owe.
    owed_releases: Option<ResMut<'w, crate::target::OwedReleases>>,
    guilds: Option<ResMut<'w, crate::ui_guild::GuildState>>,
    // The own offer the coinage watcher trims (`0x5ddf30`).
    trade: Option<ResMut<'w, crate::ui_trade::TradeSession>>,
    // The auction lists whose pages wait on an item's record, and the steps their tails take.
    auction: Option<ResMut<'w, crate::ui_auction::AuctionOpen>>,
    house: Option<ResMut<'w, crate::ui_auction::HouseLane>>,
    // The loot window's rows and the pushes that wait on an item's record.
    loot: Option<ResMut<'w, crate::ui_loot::LootState>>,
    /// What an item's deactivate lets go of besides the window above (`0x5d8660`): the loot
    /// latch, the pending record's cast and the reader.
    loot_latch: Option<ResMut<'w, crate::ui_loot::LootLatch>>,
    item_cast: crate::spell::ItemCastLeg<'w>,
    item_text: crate::ui_item_text::ItemTextLeg<'w>,
    // The group loot rolls and their lines that wait on an item's record.
    loot_rolls: crate::ui_loot_roll::RollRecordAnswers<'w>,
    // The vendor window's row and buy-back callbacks.
    merchant: crate::ui_merchant::MerchantRecordAnswers<'w>,
    // The state pass the trainer's watches and item callbacks run (`0x4d7d40`).
    trainer: crate::ui_trainer::TrainerPasses<'w>,
    // The questgiver rows' callbacks' `QUEST_ITEM_UPDATE`.
    quest_giver: Option<ResMut<'w, crate::ui_quest::QuestGiverLane>>,
    // The quest log's leaderboard's object waits' `QUEST_LOG_UPDATE`.
    quest_log: Option<ResMut<'w, crate::ui_quest_log::QuestLog>>,
    /// The quest chat lines waiting on a record, and the notices an elite accept's tutorial
    /// fires on ([`crate::ui_quest::lines`]).
    quest_lines: Option<ResMut<'w, crate::ui_quest::QuestLines>>,
    // The mail rows' and notices' callbacks' `MAIL_INBOX_UPDATE`.
    mail: Option<ResMut<'w, crate::ui_mail::MailLane>>,
    pass: ResMut<'w, update::UpdatePass>,
    released: ResMut<'w, Released>,
    nets: Query<'w, 's, &'static NetEntity>,
    list: ResMut<'w, update::ObjectList>,
    player: Option<Res<'w, crate::player::Player>>,
    item_objects: Query<'w, 's, &'static crate::items::ItemObject>,
    go_anims: Query<'w, 's, &'static mut crate::go_anim::GoAnim>,
    go_models:
        Query<'w, 's, &'static benilla_assets::ModelAnimations, With<crate::go_anim::GoAnim>>,
    pending: Query<'w, 's, (Entity, &'static Guid), With<update::ReleasePending>>,
    creatures: Option<Res<'w, crate::entities::Creatures>>,
    held_displays: Query<'w, 's, &'static mut update::HeldDisplay>,
    /// What our own `ShowHelm` and `ShowCloak` calls left on the model, which a hide bit's edge
    /// ends.
    worn_pins: Query<'w, 's, &'static mut crate::entities::WornPins>,
    /// The viewer's zone and the objects' positions a party member's snapshot stores (`0x5f0880`).
    place: crate::ui_party::net::SnapshotPlace<'w, 's>,
    /// The active mover, which its unit's release and destroy zero.
    mover: Option<ResMut<'w, super::MoverClaim>>,
    /// The far-sight machine the field's change starts or ends (`0x5de0d0`) and an arrival
    /// starts, and whose latch the stun's tick reads (`0x514540`).
    far_sight: Option<Res<'w, crate::player::FarSight>>,
    /// The active mover's auto-move, which its release and the watchers' stops end.
    follow: Option<ResMut<'w, crate::player::FollowState>>,
    approach: Option<ResMut<'w, crate::player::Approach>>,
    /// Autorun as the frame's calls leave it, which an auto-move's stop clears.
    call_autorun: Option<ResMut<'w, crate::player::CallAutorun>>,
    /// Our own control, which our arrival restores and `IsClientControlled` reads for our
    /// character (`0x5fa5f0`).
    own_control_lost: Option<ResMut<'w, crate::player::OwnControlLost>>,
    /// Who we drive and whether we may, which our own arrival gives back.
    mover_lock: Option<ResMut<'w, crate::player::MoverLock>>,
    /// The area spirit healer the ghost flag's clear empties (`0x5dea20` → `0x4921c0`), and the
    /// aura cancel it runs.
    spirit: Option<ResMut<'w, crate::ui_dialog_verbs::AreaSpiritHealer>>,
    aura: crate::ui_aura::AuraGate<'w>,
    /// The corpse-range latch the ghost bit's corpse query lowers (`0x5eeac1` → `0x491f7b`).
    death_latches: Option<ResMut<'w, crate::death::DeathLatches>>,
}

/// The item watchers' reads of the scene ([`crate::item_watch::Watch`]).
fn item_watch<'a, 'w, 's>(
    index: &'a GuidIndex,
    stores: &'a Query<'w, 's, &'static mut ObjectStore>,
    self_guid: &SelfGuid,
    items: &'a Items,
) -> crate::item_watch::Watch<'a, impl Fn(u64) -> Option<&'a ObjectFields> + use<'a, 'w, 's>> {
    crate::item_watch::Watch {
        me: self_guid.0,
        lookup: move |g| {
            index
                .0
                .get(&g)
                .and_then(|&e| stores.get(e).ok())
                .map(|s| &s.0)
        },
        items,
    }
}

/// A paper row's callback (`0x4acbf0`) on the answer for `entry`, a record or a refusal: it
/// counts one down, the last rebuilding the list off the record now cached (`0x4acbf8`).
fn paper_answered(
    In(entry): In<u32>,
    paper: Option<crate::ui_mail::StationeryRebuild>,
    items: Res<Items>,
) {
    if let Some(mut paper) = paper {
        paper.answered(entry, &items);
    }
}

/// `SMSG_ITEM_QUERY_SINGLE_RESPONSE` as `ItemCacheReceive` (`0x55bdb0`, from the handler
/// `0x555140`) runs it: the record stored, or the refusal taken, and the VM's copy with it
/// ([`crate::ui_items::feed::push_answered_record`]), then every callback the lookups that missed
/// registered, in the order they asked (`0x55ba30` links each at the head with the dedupe flag 0,
/// and `0x55bf12`-`0x55bf39` walk from the tail), each in place: what a callback signals fires at
/// this packet's place among the frame's other packets, and what it sends goes out ahead of a
/// later packet's. A refusal walks the callbacks its own add as [`crate::items::Items`]'s walk says,
/// and is asked again by the next create or entry change of that entry.
fn on_item_answer(In(ev): In<SessionEvent>, world: &mut World) {
    let SessionEvent::ItemTemplate { entry, info } = ev else {
        return;
    };
    let info = info.map(|b| *b);
    debug!("net: item template {entry} → {info:?}");
    let answered = info.is_some();
    let Some(mut items) = world.get_resource_mut::<Items>() else {
        return;
    };
    items.insert_template(entry, info);
    let callbacks = items.take_callbacks(entry);
    run_answer_step(world, crate::ui_items::feed::push_answered_record, entry);
    if answered {
        for callback in callbacks {
            run_callback(world, entry, callback, true);
        }
    } else {
        walk_refusal(world, entry, callbacks);
    }
}

/// The record answer for `entry` run as its handler runs it ([`on_item_answer`]), for a test with
/// no net stack: `info` stored, or refused, and its callbacks walked.
#[cfg(test)]
pub(crate) fn answer_item(
    world: &mut World,
    entry: u32,
    info: Option<benilla_protocol::messages::ItemInfo>,
) {
    on_item_answer(
        In(SessionEvent::ItemTemplate {
            entry,
            info: info.map(Box::new),
        }),
        world,
    );
}

/// A refusal's walk of `list` for `entry`; when a listener of what it signalled may add to it, the
/// walk goes on behind those signals ([`continue_refusal_walk`]).
fn walk_refusal(world: &mut World, entry: u32, list: Vec<crate::items::RecordCallback>) {
    let Some(items) = world.get_resource::<Items>() else {
        return;
    };
    items.begin_refusal_walk(entry, list);
    let from = items.walk_began();
    while let Some(callback) = world.resource::<Items>().next_walk_step() {
        run_callback(world, entry, callback, false);
    }
    if let (true, Some(from)) = (world.resource::<Items>().end_refusal_walk(), from) {
        crate::field_callbacks::walk_on(world, entry, from);
    }
}

/// The walk of `entry` that began at `from`, gone on behind its signals: what their listeners
/// added, walked with status 0 in the order added.
pub(crate) fn continue_refusal_walk(world: &mut World, entry: u32, from: crate::lanes::Mark) {
    let Some(items) = world.get_resource::<Items>() else {
        return;
    };
    let list = items.take_walk_tail(entry, from);
    if !list.is_empty() {
        walk_refusal(world, entry, list);
    }
}

/// One callback of an answer's list, for a record (`answered`) or a refusal.
fn run_callback(
    world: &mut World,
    entry: u32,
    callback: crate::items::RecordCallback,
    answered: bool,
) {
    {
        use crate::items::RecordCallback as Cb;
        match callback {
            // The equip callbacks run with the VM, their sends inside them.
            Cb::Swap | Cb::AutoEquip => {
                crate::ui_bind_confirm::replay_at_answer(world, entry, callback, answered)
            }
            Cb::ItemCooldown => {
                run_answer_step(world, crate::spell::net::item_cooldown_answered, entry)
            }
            // The query again for a found record only (`0x6e2f78`).
            Cb::CooldownQuery if answered => {
                run_answer_step(world, crate::spell::net::cooldown_query_answered, entry)
            }
            Cb::CooldownQuery => {}
            Cb::Stationery => run_answer_step(world, paper_answered, entry),
            // `ShowItemCastFailure 0x6e29b0`, for a record and a refusal alike: the line again,
            // on the cast lane here.
            Cb::CastFailure { spell, reason } => {
                if let Some(mut reports) = world.get_resource_mut::<crate::ui_cast::CastReports>() {
                    reports.redisplay(spell, reason, entry);
                }
            }
            // `QUEST_LOG_UPDATE` for a ready record only (`0x4e0838`), held and parked here.
            Cb::QuestLog if answered => crate::ui_items::feed::quest_log_answered(world),
            // The tooltips' rebuilds for a found record only (`0x52e551`, `0x52f826`).
            Cb::Tooltip if answered => crate::ui_items::feed::tooltips_answered(world, entry),
            Cb::QuestLog | Cb::Tooltip => {}
            // The profession and craft lists' rebuilds, for a found record only.
            Cb::TradeSkill | Cb::TradeSkillBuild | Cb::Craft => {
                if answered {
                    crate::ui_tradeskill::record_answered(world, callback);
                }
            }
            // The try-on again, for a found record only (`0x504ba8`).
            Cb::TryOn if answered => crate::ui_dressup::try_on_answered(world, entry),
            Cb::TryOn => {}
            // The combat log's, for a record and a refusal alike (`0x6294b0`): the last waiting
            // entry of its kind and item freed, and for a record its formatter run again, its
            // line on the chat lane here.
            Cb::CombatLog(kind) => run_answer_step(
                world,
                crate::combat_log::item_answered,
                (kind, entry, answered),
            ),
            _ => run_answer_step(world, run_record_callback, (entry, callback, answered)),
        }
    }
}

/// Run one step of an answer's callbacks as a one-shot, its unit edits applied as it returns; a
/// harness without its state runs nothing.
fn run_answer_step<T: 'static, M>(
    world: &mut World,
    step: impl IntoSystem<In<T>, (), M> + 'static,
    input: T,
) {
    if let Err(err) = world.run_system_cached_with(step, input) {
        debug!("net: an item answer's callback did not run: {err}");
    }
    crate::creature_anim::apply_unit_edits(world);
}

/// One callback an item lookup registered, for the answer of `entry`, a record (`answered`) or a
/// refusal: each item's set-up (`0x5d8390`) and entry change's (`0x5d96b0`), the loot window's rows
/// (`0x4c2ac0`) and pushes (`0x491ee0`), the trade getters' (`0x4c0340`), the ammo getters'
/// (`0x4c84f0`), the vendor rows' (`0x4fada0`, `0x4fb570`), the trainer rows' (`0x4d9140`,
/// `0x4d9db0`), the questgiver rows' (`0x5018c0`), the turn-in's reward lines' (`0x5dc7b0`), the
/// mail rows' and notices' (`0x4af0a0`, `0x4ad010`), the mail attachment's (`0x4ae710`), the
/// auction notices' lines (`0x4cd190`) and the auction lists' (`0x4cca10`, `0x4ccc60`,
/// `0x4cceb0`). The equip callbacks are the VM's
/// ([`crate::ui_bind_confirm::replay_at_answer`]).
fn run_record_callback(
    In((entry, callback, answered)): In<(u32, crate::items::RecordCallback, bool)>,
    mut sc: Scene,
) {
    use crate::items::RecordCallback as Cb;
    match callback {
        // A refusal finishes no set-up (its found byte is clear, `0x5d83b8`) and runs
        // no entry change (`0x5d96bb`).
        Cb::SetUp(_) | Cb::Entry(_) if !answered => {}
        Cb::SetUp(guid) => {
            let run = item_watch(&sc.index, &sc.stores, &sc.self_guid, &sc.items).set_up_run(
                guid,
                entry,
                &sc.bag_cache,
            );
            if run.notice.is_some() {
                replay_enchant_times(
                    &mut sc.items,
                    &mut sc.countdowns,
                    &sc.index,
                    &mut sc.view_log,
                    guid,
                );
                // Its entry sorts ahead of its own notice and behind the set-ups before it.
                sc.view_log.close();
            }
            sc.item_lane.push(run);
        }
        Cb::Entry(guid) => {
            let run = item_watch(&sc.index, &sc.stores, &sc.self_guid, &sc.items).entry_run(
                guid,
                entry,
                &sc.bag_cache,
            );
            sc.item_lane.push(run);
        }
        // A refusal releases a row ([`crate::ui_loot`]'s deviation).
        Cb::LootRow => {
            if let Some(loot) = sc.loot.as_mut() {
                loot.row_answered(&sc.items);
            }
        }
        Cb::Push => {
            if let Some(loot) = sc.loot.as_mut() {
                loot.push_answered(&sc.items, entry);
            }
        }
        // The trade and the ammo getters' callbacks run for a ready record only
        // (`0x4c0343`, `0x4c84f6`).
        Cb::Trade if answered => sc.item_lane.push(crate::item_watch::Run {
            notice: Some(crate::item_watch::ItemNotice::TradeRecord { entry }),
            ..Default::default()
        }),
        Cb::Ammo if answered => sc
            .field_notices
            .write(crate::field_watch::FieldNotice::Player {
                id: crate::field_watch::id::UNIT_INVENTORY_CHANGED,
            }),
        Cb::Trade | Cb::Ammo => {}
        // The bar's recount of the entry, for a ready record only (`0x4e5b86` → `0x4e5c70`).
        Cb::ActionSlots if answered => sc.item_lane.push(crate::item_watch::Run {
            notice: Some(crate::item_watch::ItemNotice::ActionRecord { entry }),
            ..Default::default()
        }),
        Cb::ActionSlots => {}
        // The game object plate's rebuild, for a found record only (`0x52ad2f`).
        Cb::LockKey if answered => sc.item_lane.push(crate::item_watch::Run {
            notice: Some(crate::item_watch::ItemNotice::LockKeyRecord),
            ..Default::default()
        }),
        Cb::LockKey => {}
        Cb::Swap
        | Cb::AutoEquip
        | Cb::ItemCooldown
        | Cb::CooldownQuery
        | Cb::Stationery
        | Cb::CastFailure { .. }
        | Cb::QuestLog
        | Cb::Tooltip
        | Cb::TradeSkill
        | Cb::TradeSkillBuild
        | Cb::Craft
        | Cb::TryOn
        | Cb::CombatLog(_) => {}
        // The vendor's run for a record and a refusal alike (`0x4fb0f0`, `0x4fb570`).
        Cb::Merchant => sc.merchant.row_answered(),
        Cb::Buyback => sc.merchant.buyback_answered(),
        // The trainer getters' run the state pass for a ready record only (`0x4d9146`,
        // `0x4d9db6`).
        Cb::Trainer if answered => sc.trainer.owed(),
        Cb::Trainer => {}
        // `QUEST_ITEM_UPDATE` for a ready record only (`0x5018d6`, `0x5018f5`,
        // `0x501903`), whatever the window shows then.
        Cb::Quest if answered => {
            if let Some(lane) = sc.quest_giver.as_mut() {
                lane.push(crate::ui_quest::QuestGiverEvent::ItemUpdate);
            }
        }
        Cb::Quest => {}
        // The turn-in's reward line, shown with a ready record (`0x5dc7b0`).
        Cb::QuestReward => {
            if let (Some(lines), Some(lane)) = (sc.quest_lines.as_mut(), sc.quest_giver.as_mut()) {
                lines.item_answered(entry, &sc.items, lane);
            }
        }
        // `MAIL_INBOX_UPDATE` for a ready record only (`0x4af0a6`, `0x4ad016`); the
        // notice's subject reads the record as its row resolves.
        Cb::Mail if answered => {
            if let Some(lane) = sc.mail.as_mut() {
                lane.push(crate::ui_mail::MailEvent::InboxUpdate);
            }
        }
        Cb::Mail => {}
        // An auction notice's line, shown with the name or, for a refusal, dropped
        // (`0x4cd190`).
        Cb::AuctionMessage => {
            if let (Some(auction), Some(house)) = (sc.auction.as_mut(), sc.house.as_mut()) {
                crate::ui_auction::message_answered(auction, house, &sc.items, entry);
            }
        }
        // `MAIL_SEND_INFO_UPDATE` for a ready record only (`0x4ae716`, `0x4ae71f`).
        Cb::SendMail if answered => {
            if let Some(lane) = sc.mail.as_mut() {
                lane.push(crate::ui_mail::MailEvent::SendInfoUpdate);
            }
        }
        Cb::SendMail => {}
        // `NEW_AUCTION_UPDATE` for a ready record only (`0x4ce2f6`-`0x4ce2ff`).
        Cb::AuctionSellItem if answered => {
            if let Some(house) = sc.house.as_mut() {
                crate::ui_auction::sell_item_answered(house);
            }
        }
        Cb::AuctionSellItem => {}
        // A list's, for a record and a refusal alike (`0x4cca10`, `0x4ccc60`, `0x4cceb0`).
        Cb::AuctionList(list) => {
            if let (Some(auction), Some(house)) = (sc.auction.as_mut(), sc.house.as_mut()) {
                crate::ui_auction::list_record_answered(auction, house, list);
            }
        }
        // A roll's or its line's, for a record and a refusal alike: the start's opens the roll
        // (`0x61b460` → `0x61b430`), a line's goes on to its name or is shown (`0x61c4b0`,
        // `0x61bd50`, `0x61b7d0`).
        Cb::LootRoll(wait) => sc.loot_rolls.item_answered(
            wait,
            answered,
            &sc.items,
            &sc.names,
            &sc.net,
            sc.self_guid.0,
        ),
    }
}

/// Items past their create block: the lifetime and enchant countdowns, written into the item's
/// own [`crate::items::Countdowns`].
fn on_item(In(ev): In<SessionEvent>, mut sc: Scene) {
    match ev {
        // `0x5e4f30`'s two arms share the item lookup (`0x468460`, `TYPEMASK_ITEM`) and differ on
        // a miss.
        SessionEvent::ItemTime { item_guid, seconds } => {
            // An unheld item drops the update, with no queue (`0x1EA`'s arm returns), so an item
            // created after the login re-send (`Player::SendItemDurations`) waits for the next.
            match sc.index.0.get(&item_guid).copied() {
                Some(e) if sc.countdowns.contains(e) => {
                    if let Ok(mut c) = sc.countdowns.get_mut(e) {
                        sc.view_log.countdowns(item_guid, Some(c.clone()));
                        c.set_lifetime(seconds);
                    }
                    debug!("item duration: item {item_guid:#x} → {seconds}s");
                }
                _ => debug!("item duration: {item_guid:#x} names an item we do not hold — dropped"),
            }
        }
        SessionEvent::ItemEnchantTime {
            item_guid,
            slot,
            seconds,
        } => match sc.index.0.get(&item_guid).copied() {
            Some(e) if sc.countdowns.contains(e) => {
                if let Ok(mut c) = sc.countdowns.get_mut(e) {
                    sc.view_log.countdowns(item_guid, Some(c.clone()));
                    if c.set_enchant(slot, seconds) {
                        debug!("enchant timer: item {item_guid:#x} slot {slot} → {seconds}s");
                    } else {
                        debug!("enchant timer: item {item_guid:#x} slot {slot} is past the seventh — refused");
                    }
                }
            }
            // `0x1EB`'s miss arm queues on a resolving player's list (`0x5ebd40`), and only the
            // active player's is replayed (`0x5d8440` → `0x5ebde0`). vmangos names the owner, us
            // (`Player.cpp:11883`, `:12077`), so it queues on the active player.
            _ if sc.self_guid.0.is_some_and(|g| sc.index.0.contains_key(&g)) => {
                sc.items.queue_enchant_time(item_guid, slot, seconds)
            }
            _ => debug!("enchant timer: {item_guid:#x} unheld and no player to queue on — dropped"),
        },
        _ => {}
    }
    sc.view_log.close();
}

/// `CGItem_C::ApplyRecord`'s replay (`0x5ebde0` at `0x5d85b7`): the enchant times queued on the
/// active player while the item was not held, set as of now. The record set-up reaches it only
/// for our item while in the world (`0x5d8504`, `0x5d8521`), as [`ItemNotice::Record`] runs.
///
/// [`ItemNotice::Record`]: crate::item_watch::ItemNotice::Record
fn replay_enchant_times(
    items: &mut Items,
    countdowns: &mut Query<&mut crate::items::Countdowns>,
    index: &GuidIndex,
    view_log: &mut crate::ui_unit::views::ViewLog,
    guid: u64,
) {
    let queued = items.take_enchant_times(guid);
    let Some(mut cells) = index.0.get(&guid).and_then(|&e| countdowns.get_mut(e).ok()) else {
        return;
    };
    if !queued.is_empty() {
        view_log.countdowns(guid, Some(cells.clone()));
    }
    for (slot, seconds) in queued {
        if !cells.set_enchant(slot, seconds) {
            debug!("enchant timer: queued slot {slot} for {guid:#x} is past the seventh — refused");
        }
    }
}

/// An observed mover skipped time: only its relay chain moves, as the reference's handler does
/// (`0x603b40` → `0x61ab90`, `[CMovement+0xac] += lag`); an unknown guid is dropped, as there.
fn on_move_time_skipped(In(ev): In<SessionEvent>, mut sc: Scene) {
    if let SessionEvent::MoveTimeSkipped { guid, lag_ms } = ev {
        let Some(&e) = sc.index.0.get(&guid) else {
            return;
        };
        if let Ok(mut m) = sc.remote.get_mut(e) {
            m.relay.skip_time(lag_ms);
        } else if let Ok(mut seed) = sc.seeds.get_mut(e) {
            seed.chain.skip_time(lag_ms);
        }
    }
}

/// A relayed `MSG_MOVE_*` ([`unit_move`]); `world`, absent in an app with no world, is what a due
/// arrival's catch-up resolves against.
fn on_unit_move(In(ev): In<SessionEvent>, mut sc: Scene, world: Option<MoverWorld>) {
    if let SessionEvent::UnitMove {
        guid,
        position,
        orientation,
        flags,
        pitch,
        time,
        verb,
        fall_time,
        jump,
        transport,
    } = ev
    {
        let now_ms = sc.real.elapsed_secs_f64() * 1000.0;
        let tick = sc.move_tick.as_deref().and_then(|t| t.0);
        unit_move(
            guid,
            crate::net::motion::RelayMove {
                wire_ms: time,
                position,
                orientation,
                flags,
                pitch,
                fall_time,
                jump,
                transport,
                verb,
            },
            now_ms,
            tick,
            &mut sc.commands,
            &sc.index,
            &sc.self_guid,
            &mut sc.remote,
            &sc.seeds,
            &mut sc.transforms,
            &sc.stores,
            &sc.speeds,
            &sc.modes,
            world.as_ref(),
            &mut sc.hard_landings,
            &mut sc.self_moves,
        );
    }
}

fn on_monster_move(In(ev): In<SessionEvent>, mut sc: Scene) {
    if let SessionEvent::MonsterMove {
        guid,
        transport,
        start,
        spline_id,
        path,
        facing,
        stop,
        duration_ms,
        flying,
        run_mode,
    } = ev
    {
        let rooted = modes_of(guid, &sc.index, &sc.modes).rooted();
        let smooth =
            sc.index.0.get(&guid).is_some_and(|&e| {
                smoothed(sc.nets.get(e).ok().map(|n| n.kind), sc.movers.contains(e))
            });
        monster_move(
            guid,
            transport,
            start,
            spline_id,
            path,
            facing,
            stop,
            duration_ms,
            flying,
            run_mode,
            rooted,
            smooth,
            &mut sc.commands,
            &sc.index,
            &mut sc.transforms,
            &mut sc.riders,
        );
        // A path started on the active mover runs its input tick (`0x6187a0` → `0x602053`), whose
        // precondition the server's spline fails (`0x5144e0`): the teardown leg stops the auto-move
        // (`0x5146d6`). A stop starts no path, and nor does a rooted mover: the start returns 0 on
        // the root bit `0x1000` (`0x6187c2`-`0x6187c7`), so `0x601fee` skips the tick.
        let mover_rooted = sc
            .player
            .as_deref()
            .is_some_and(crate::player::Player::rooted);
        if !stop && !mover_rooted {
            update::stop_auto_move(
                sc.mover.as_deref(),
                sc.follow.as_deref_mut(),
                sc.approach.as_deref_mut(),
                sc.call_autorun.as_deref_mut(),
                &mut sc.field_notices,
                guid,
            );
        }
    }
}

fn on_gameobject_info(In(ev): In<SessionEvent>, mut sc: Scene) {
    if let SessionEvent::GameObjectInfo {
        entry,
        type_id,
        display_id,
        name,
        data,
    } = ev
    {
        gameobject_info(
            entry,
            type_id,
            display_id,
            name,
            &data,
            &mut sc.go_templates,
        );
        // The leaderboard's callback `0x4e0800` signals for a landed record (`0x4e081a`).
        if sc.go_templates.take_quest_wait(entry) {
            if let Some(log) = sc.quest_log.as_mut() {
                log.ready(crate::ui_quest_log::QuestEvent::Named);
            }
        }
    }
}

/// A refused entry: the template cache frees it (`0x5588e0`'s refusal leg), so no zeroed template
/// stands in for the object.
fn on_gameobject_unknown(In(ev): In<SessionEvent>, mut sc: Scene) {
    if let SessionEvent::GameObjectUnknown { entry } = ev {
        sc.go_templates.refuse(entry);
        // A refusal runs the leaderboard's callback with status 0, which only logs (`0x4e080a`).
        sc.go_templates.take_quest_wait(entry);
    }
}

fn on_gameobject_anim(In(ev): In<SessionEvent>, mut sc: Scene) {
    match ev {
        SessionEvent::GameObjectCustomAnim { guid, anim_id } => {
            debug!("net: gameobject {guid:#x} custom anim {anim_id}");
            gameobject_anim_event(guid, &sc.index, &mut sc.go_anims, |go| go.custom(anim_id));
        }
        SessionEvent::GameObjectDespawnAnim { guid } => {
            debug!("net: gameobject {guid:#x} despawn anim");
            gameobject_anim_event(guid, &sc.index, &mut sc.go_anims, |go| go.despawn());
        }
        _ => {}
    }
}

/// The observer movement-mode family: no ack.
fn on_spline_move_mode(In(ev): In<SessionEvent>, mut sc: Scene) {
    if let SessionEvent::SplineMoveMode { guid, mode, apply } = ev {
        spline_move_mode(
            guid,
            mode,
            apply,
            &mut sc.commands,
            &sc.index,
            &mut sc.modes,
            &mut sc.remote,
        );
    }
}

fn on_speed(In(ev): In<SessionEvent>, mut sc: Scene) {
    match ev {
        SessionEvent::ForceSpeedChange {
            guid,
            kind,
            counter,
            speed,
        } => force_speed_change(
            guid,
            kind,
            counter,
            speed,
            &sc.index,
            &mut sc.speeds,
            &sc.self_guid,
            &mut sc.speed_changes,
        ),
        SessionEvent::SpeedChanged { guid, kind, speed } => {
            speed_changed(guid, kind, speed, &sc.index, &mut sc.speeds)
        }
        _ => {}
    }
}

/// A placed object's create block past its fields: what it is, where, and how it moves.
pub(super) struct CreateBlock {
    pub(super) guid: u64,
    pub(super) kind: EntityKind,
    pub(super) display_id: Option<u32>,
    pub(super) position: [f32; 3],
    pub(super) orientation: f32,
    pub(super) scale: f32,
    pub(super) speeds: Option<MoveSpeeds>,
    pub(super) mover: Option<benilla_protocol::MoverState>,
    pub(super) transport_progress: Option<benilla_protocol::TransportClock>,
    pub(super) transport: Option<benilla_protocol::TransportPose>,
    pub(super) spline: Option<benilla_protocol::CreateSpline>,
}

/// Where a create block's object lands.
pub(super) enum Landing {
    /// A held object, refreshed in place; the caller writes its store.
    Live(Entity),
    /// A new entity with `store`: an object allocated now, or a released one brought back, which
    /// keeps `kept_pose` when its type takes no position from the block, and its clock.
    New {
        store: ObjectFields,
        kept_pose: Option<Transform>,
        kept_anchor: Option<crate::transport::TransportAnchor>,
    },
}

/// An `SMSG_UPDATE_OBJECT` create block's placement: spawns or refreshes the entity and warms the
/// ask-once caches off the block's `fields`. Returns the entity.
fn object_create(
    sc: &mut Scene,
    block: CreateBlock,
    fields: &ObjectFields,
    landing: Landing,
) -> Entity {
    let CreateBlock {
        guid,
        kind,
        display_id,
        position,
        orientation,
        scale,
        speeds,
        mover,
        transport_progress,
        transport,
        spline,
    } = block;
    let net = NetEntity {
        kind,
        display_id,
        scale,
    };
    // A transport's cycle anchor: the create block's `UPDATE_FLAG_TRANSPORT` u32 and the instant
    // its packet arrived (`0x5f7080` subtracts the queue node's stamp); a re-create re-anchors. The reference's per-frame tick (`0x630970`) runs types 15
    // (boats, a path-progress clock) and 11 (lifts, `time-since-create % period`,
    // `GameObject.cpp:246`).
    let go_type = (kind == EntityKind::GameObject).then(|| fields.gameobject_type_id());
    let transport_anchor = matches!(go_type, Some(11 | 15))
        .then_some(transport_progress)
        .flatten()
        .map(|clock| crate::transport::TransportAnchor {
            progress_ms: clock.progress_ms,
            at: clock.arrived.unwrap_or_else(std::time::Instant::now),
        });
    // The placement, and on a type-11 lift the basis its keyframe offsets rotate through.
    let go_quat = (kind == EntityKind::GameObject)
        .then(|| fields.gameobject_rotation())
        .flatten();
    // A type-11 lift's seed: keyframes keyed by the entry, rotated by the quat, based at the
    // create position. One whose create carried no progress u32 has no clock and stays frozen.
    let elevator_seed = (matches!(go_type, Some(11)) && transport_anchor.is_some())
        .then(|| fields.object_entry())
        .flatten()
        .map(|entry| crate::transport::ElevatorSeed {
            entry,
            base_pos: position,
            yaw: orientation,
            quat: go_quat,
        });
    // A GameObject is placed by its quaternion. A mover's create block also seeds its flags
    // (`0x618c30`, merge mask `0x75a07dff`) and pitch (`0x7c6420`) before the model is built
    // (`0x613e10`), so a swimmer that streams in nose-down is drawn nose-down from the first frame.
    let placement = match kind {
        EntityKind::GameObject => gameobject_rotation(go_quat, orientation),
        _ => mover.map_or_else(
            || wire_yaw(orientation),
            |m| crate::creature_anim::swim_body_rotation(orientation, m.flags, m.pitch),
        ),
    };
    // A unit created on a transport: the living block's rider tail is its local pose, which
    // `compose_riders` carries through the boat each frame; the world `position` is the fallback.
    let rider = matches!(kind, EntityKind::Unit | EntityKind::Player)
        .then_some(transport)
        .flatten()
        .map(|t| crate::transport::TransportRider {
            transport_guid: t.guid,
            local_pos: [t.pos.x, t.pos.y, t.pos.z],
            local_orientation: t.orientation,
        });
    // Aboard a held transport the unit stands where its tail puts it on the deck, from this
    // block on: the create's apply re-seats it deck-local as a relayed move's does (`0x619320` →
    // `0x618c30`, `0x618e07`).
    let seat = rider
        .as_ref()
        .and_then(|_| aboard(transport.as_ref(), &sc.index, &sc.transforms, &sc.stores));
    let (position, placement) = match seat {
        Some((world, yaw)) => (bevy_to_wow(world), Quat::from_rotation_y(yaw)),
        None => (position, placement),
    };
    if matches!(kind, EntityKind::GameObject) {
        // Where and how big, under `RUST_LOG=benilla_app::net::objects=debug`.
        debug!(
            "gameobject spawn: entry {:?} display {display_id:?} type {go_type:?} \
             pos [{:.2}, {:.2}, {:.2}] scale {scale}",
            fields.object_entry(),
            position[0],
            position[1],
            position[2],
        );
    }
    // The walk this unit is already on, joined at the server's progress. Traced first, so a
    // `WOW_CREATE_SPLINE=off` run still records what the wire offered.
    trace_create_spline(guid, spline.as_ref());
    let walk = spline.and_then(create_spline);
    // A unit's status applies forced (`0x619320` → `0x618c30`), re-seeding the mover's replay
    // chain from the block's stamp and the clock; a held unit's ring is its own, kept.
    let seed = matches!(kind, EntityKind::Unit | EntityKind::Player)
        .then_some(mover)
        .flatten()
        .filter(|_| sc.self_guid.0 != Some(guid))
        .map(|m| {
            let held = match &landing {
                Landing::Live(e) => sc
                    .remote
                    .get(*e)
                    .map(|rm| rm.relay.clone())
                    .or_else(|_| sc.seeds.get(*e).map(|s| s.chain.clone()))
                    .ok(),
                Landing::New { .. } => None,
            };
            let mut chain = held.unwrap_or_default();
            chain.seed_at_create(m.time, sc.real.elapsed_secs_f64() * 1000.0);
            RelaySeed {
                chain,
                flags: crate::creature_anim::move_flags::merge_server_authored(0, m.flags),
            }
        });
    let commands = &mut sc.commands;
    match landing {
        Landing::Live(e) => {
            // A re-create is a fresh snapshot: refresh identity and a unit's pose, drop any
            // extrapolation; the mover's chain goes on in the seed.
            commands.entity(e).insert(net).remove::<RemoteMotion>();
            match seed {
                Some(seed) => {
                    commands.entity(e).insert(seed);
                }
                None => {
                    commands.entity(e).remove::<RelaySeed>();
                }
            }
            // The set is replaced whole; a `SMSG_FORCE_*_SPEED_CHANGE` in the same tick lands on
            // top, since this insert applies before the next packet's handler
            // (`apply::seam_tests`).
            if let Some(s) = speeds {
                commands.entity(e).insert(UnitSpeeds(s));
            }
            // `0x466350` constructs nothing, so a held transport keeps its clock; one with none is
            // the ship kept across a worldport, standing for the new object built there.
            if !sc.anchors.contains(e) {
                if let Some(anchor) = transport_anchor {
                    commands.entity(e).insert(anchor);
                }
            }
            if let Some(seed) = elevator_seed {
                commands.entity(e).insert(seed);
            }
            match rider {
                Some(r) => {
                    commands.entity(e).insert(r);
                }
                None => {
                    commands
                        .entity(e)
                        .remove::<crate::transport::TransportRider>();
                }
            }
            // A create without a path says the unit stands still, so a stale path goes.
            match walk {
                Some(s) => {
                    commands.entity(e).insert(s);
                }
                None => {
                    commands.entity(e).remove::<Spline>();
                }
            }
            // Only a unit takes the block's movement (type-mask bit 3, `0x46638c`); any other type
            // reads the position and drops it (`0x4664c3`..`0x46656b`), so it stays where it
            // stands.
            if matches!(kind, EntityKind::Unit | EntityKind::Player) {
                write_pose(commands, &mut sc.transforms, e, position, placement);
            }
            e
        }
        Landing::New {
            store,
            kept_pose,
            kept_anchor,
        } => {
            // A transport spawns hidden: its create pose is the spawn point, not where it is in
            // its cycle; its first tick unhides it, posed unless its leg is on another map.
            let visibility = if transport_anchor.is_some() {
                Visibility::Hidden
            } else {
                Visibility::default()
            };
            let mut entity = commands.spawn((
                Guid(guid),
                net,
                kept_pose.unwrap_or_else(|| pose_transform(position, placement)),
                visibility,
                ObjectStore(store),
                sc.list.link(),
            ));
            // Chain-only visibility: the root renders nothing itself, and culling flips its
            // `Visibility` through `Mut` writes, which do not re-add the sweep row.
            entity.vis_chain_only();
            if let Some(s) = speeds {
                entity.insert(UnitSpeeds(s));
            }
            // Reactivation constructs nothing either (`0x464530`): a brought-back clock stands.
            if let Some(anchor) = kept_anchor.or(transport_anchor) {
                entity.insert(anchor);
            }
            if let Some(seed) = elevator_seed {
                entity.insert(seed);
            }
            if let Some(r) = rider {
                entity.insert(r);
            }
            if let Some(s) = walk {
                entity.insert(s);
            }
            if let Some(seed) = seed {
                entity.insert(seed);
            }
            // Our own player is the active one from its write: the reference finds it by its guid
            // (`0x468550`), here by the tag, given in the same commands as the entity.
            if sc.self_guid.0 == Some(guid) {
                entity.insert(SelfPlayer);
            }
            let e = entity.id();
            sc.index.0.insert(guid, e);
            crate::target::object_held(sc.selection.as_mut(), guid, e);
            e
        }
    }
}

/// An `SMSG_UPDATE_OBJECT` movement block: a new pose that supersedes any active path. The unit
/// this client moves, our player or a possessed unit ([`ActiveMover`]), drops it: the reference
/// compares the block's guid with the active mover before it looks the object up (`0x4670a5`,
/// `0x4670ae`, `0x4670b5`), and returns on a match (`0x4670bb`).
pub(super) fn object_move(
    guid: u64,
    position: [f32; 3],
    orientation: f32,
    commands: &mut Commands,
    index: &GuidIndex,
    transforms: &mut Query<&mut Transform>,
    movers: &Query<(), With<ActiveMover>>,
) {
    if let Some(&e) = index.0.get(&guid) {
        if movers.contains(e) {
            return;
        }
        commands.entity(e).remove::<Spline>();
        // A yaw only: the GameObjects that get one are transports, which their tick then owns.
        write_pose(commands, transforms, e, position, wire_yaw(orientation));
    }
}

/// `GAMEOBJECT_FLAGS`' transport bit, which `0x630a80` tests (`shr 3; and 1`).
const GO_FLAG_TRANSPORT: u32 = 0x8;

/// Where a status aboard a held transport puts its mover, `None` off one: the apply re-seats the
/// mover deck-local inside the handler when the status's transport resolves (`0x7c6900`, through
/// `0x618b30` at `0x618e07`), and a unit's position reads that through the deck as it stands
/// ([`crate::transport::on_deck`]); an unresolved transport leaves the world pose (`0x7c692b`).
/// It resolves to a held game object whose `GAMEOBJECT_FLAGS` has the transport bit, bit 3, and
/// nothing more (`0x630a80`): a deck created earlier in the frame, which the tick has not armed
/// yet, takes its rider where its create stands it.
fn aboard(
    transport: Option<&benilla_protocol::TransportPose>,
    index: &GuidIndex,
    transforms: &Query<&mut Transform>,
    stores: &Query<&mut ObjectStore>,
) -> Option<(Vec3, f32)> {
    let t = transport?;
    let deck = index.0.get(&t.guid).copied()?;
    let fields = &stores.get(deck).ok()?.0;
    let is_deck = fields.object_type() == Some(benilla_protocol::messages::ObjectType::GameObject)
        && fields.gameobject_flags() & GO_FLAG_TRANSPORT != 0;
    if !is_deck {
        return None;
    }
    let deck = transforms.get(deck).ok()?;
    Some(crate::transport::on_deck(
        deck,
        [t.pos.x, t.pos.y, t.pos.z],
        t.orientation,
    ))
}

/// What a due arrival's catch-up steps its mover with ([`crate::net::motion::catch_up`]): the ms
/// the movement clock is past the move's fire-time, the unit's speeds and granted modes, and the
/// world.
struct Lateness<'a, 'w, 's> {
    late_ms: f64,
    speeds: MoveSpeeds,
    granted: u32,
    world: Option<&'a MoverWorld<'w, 's>>,
}

/// A due relayed move's pose on its unit, inside the handler: the apply writes the mover's
/// position at the packet (`0x7c6420` at `0x618dfc`), the late start's catch-up steps it on by
/// its lateness (`0x618bb0` at `0x618e5d`), and a unit's position reads the result at the call
/// (`0x5f1f10`), so a listener later in the frame reads the unit where this packet put it. Aboard,
/// the composed pose, as [`crate::transport::compose_riders`] carries it; the display facing and
/// the frame's dead-reckoning stay frame time ([`crate::net::extrapolate_remote_units`]).
fn seat_arrival(
    e: Entity,
    mv: &crate::net::motion::RelayMove,
    rm: &mut RemoteMotion,
    late: Lateness,
    index: &GuidIndex,
    transforms: &mut Query<&mut Transform>,
    stores: &Query<&mut ObjectStore>,
) {
    let deck = aboard(mv.transport.as_ref(), index, transforms, stores);
    let Ok(mut t) = transforms.get_mut(e) else {
        return;
    };
    match deck {
        Some((world, yaw)) => {
            t.translation = world;
            t.rotation = Quat::from_rotation_y(yaw);
            rm.wow_pos = bevy_to_wow(world);
            rm.orientation = yaw;
        }
        None => {
            let modes = rm.flags | late.granted;
            let riding = mv.transport.is_some();
            catch_up(e, rm, late.late_ms, late.speeds, modes, riding, late.world);
            let at = benilla_assets::coords::wow_to_bevy(rm.wow_pos);
            if t.translation != at {
                t.translation = at;
            }
        }
    }
}

/// A relayed `MSG_MOVE_*`. The reference schedules a remote's apply (`0x618c30`): the mover's
/// replay chain gives the packet a fire-time, a move due by the movement clock the last tick
/// stamped (`tick`, `@0x618dd2`) applies now and any other queues for `drain_pending_moves`, this
/// frame's tick, with dead-reckoning in between. `WOW_REMOTE_SNAP=1` applies at arrival.
fn unit_move(
    guid: u64,
    mv: crate::net::motion::RelayMove,
    now_ms: f64,
    tick: Option<f64>,
    commands: &mut Commands,
    index: &GuidIndex,
    self_guid: &SelfGuid,
    remote_motion: &mut Query<&mut RemoteMotion>,
    seeds: &Query<&mut RelaySeed>,
    transforms: &mut Query<&mut Transform>,
    stores: &Query<&mut ObjectStore>,
    speeds: &Query<&mut UnitSpeeds>,
    modes: &Query<&mut UnitMoveModes>,
    world: Option<&MoverWorld>,
    landings: &mut MessageWriter<crate::creature_anim::HardLanding>,
    self_moves: &mut MessageWriter<crate::net::SelfMoveMessage>,
) {
    use crate::net::motion::{apply_move, arrival_snap, trace_relay, PendingMove, RelayOutcome};
    // Our own guid: the server writing our pose (`SetAsServerSide`, `ctime = 0`), which the
    // reference applies with no mover gate (`0x603bb0`); it goes to `player::wire_in`.
    if self_guid.0 == Some(guid) {
        trace_relay(
            guid,
            &mv,
            &Default::default(),
            now_ms,
            0,
            RelayOutcome::SelfMover,
        );
        self_moves.write(crate::net::SelfMoveMessage {
            position: mv.position,
            orientation: mv.orientation,
            flags: mv.flags,
            pitch: mv.pitch,
            fall_time: mv.fall_time,
            jump: mv.jump,
            transport: mv.transport,
            teleport: mv.verb == benilla_protocol::RelayVerb::Teleport,
        });
        return;
    }
    let Some(&e) = index.0.get(&guid) else {
        // No entity: traced, since a mover running on while these pile up is a streaming bug.
        trace_relay(
            guid,
            &mv,
            &Default::default(),
            now_ms,
            0,
            RelayOutcome::Unknown,
        );
        return;
    };
    // What a due arrival's catch-up steps with: the ms the clock is past the fire-time, none
    // before the first tick.
    let late = |fire_ms: f64| Lateness {
        late_ms: tick.map_or(0.0, |t| t - fire_ms),
        speeds: speeds
            .get(e)
            .map_or_else(|_| MoveSpeeds::default(), |u| u.0),
        granted: modes.get(e).map_or(0, |m| m.0),
        world,
    };
    {
        // The server's pose supersedes a stale spline.
        commands.entity(e).remove::<Spline>();
        if let Ok(mut rm) = remote_motion.get_mut(e) {
            // The chain reads the state before this move applies, as the reference times the
            // packet off the live `[esi+0x40]`/`[esi+0x150]`.
            let (flags, queue_empty) = (rm.flags, rm.pending.is_empty());
            let fire_ms = rm.relay.schedule(mv.wire_ms, now_ms, flags, queue_empty);
            let at_arrival = arrival_snap() || rm.fires_at_arrival(fire_ms, tick);
            trace_relay(
                guid,
                &mv,
                &rm.relay,
                now_ms,
                rm.pending.len(),
                if at_arrival {
                    RelayOutcome::Now
                } else {
                    RelayOutcome::Queued
                },
            );
            if at_arrival {
                apply_move(e, &mv, &mut rm, now_ms, commands, landings);
                seat_arrival(e, &mv, &mut rm, late(fire_ms), index, transforms, stores);
            } else {
                // Fire-times are monotone per unit, so the queue stays ordered.
                rm.pending.push_back(PendingMove { fire_ms, mv });
            }
        } else {
            // The first move builds the unit's motion where it stands, on what its create left:
            // the chain and the flags word its schedule reads as the mover's ([`RelaySeed`]). A
            // unit whose create carried no status seeds the chain here, its fire-time the arrival
            // (`@0x618c54`), past the movement clock, so it waits for this frame's tick like any
            // idle mover's, unless the lever applies it at once, its facing shown then.
            let seed = seeds.get(e).ok().cloned();
            let created = seed.is_some();
            let (relay, flags) = seed.map_or_else(
                || (crate::net::motion::RelayChain::default(), 0),
                |s| (s.chain, s.flags),
            );
            let (wow_pos, orientation) =
                transforms
                    .get(e)
                    .map_or((mv.position, mv.orientation), |t| {
                        (
                            bevy_to_wow(t.translation),
                            t.rotation.to_euler(EulerRot::YXZ).0,
                        )
                    });
            let mut rm = RemoteMotion {
                wow_pos,
                orientation,
                flags,
                pitch: 0.0,
                speed: 0.0,
                vertical_velocity: 0.0,
                jump_xy_vel: [0.0; 2],
                fall_start: None,
                pending: std::collections::VecDeque::new(),
                relay,
                last_apply_ms: now_ms,
                last_apply_pos: wow_pos,
            };
            let fire_ms = rm.relay.schedule(mv.wire_ms, now_ms, flags, true);
            debug_assert!(
                created || fire_ms == now_ms,
                "a seeding packet's fire-time is its arrival"
            );
            if arrival_snap() || rm.fires_at_arrival(fire_ms, tick) {
                let outcome = if created {
                    RelayOutcome::Now
                } else {
                    RelayOutcome::Seed
                };
                trace_relay(guid, &mv, &rm.relay, now_ms, 0, outcome);
                apply_move(e, &mv, &mut rm, now_ms, commands, landings);
                write_pose(
                    commands,
                    transforms,
                    e,
                    mv.position,
                    wire_yaw(mv.orientation),
                );
                seat_arrival(e, &mv, &mut rm, late(fire_ms), index, transforms, stores);
            } else {
                trace_relay(guid, &mv, &rm.relay, now_ms, 0, RelayOutcome::Queued);
                rm.pending.push_back(PendingMove { fire_ms, mv });
            }
            commands.entity(e).insert(rm).remove::<RelaySeed>();
        }
    }
}

/// An object leaving the live table, released or destroyed (`0x464920`): its detached model fades
/// out ([`DespawnFade`]); its OnDeactivate (`0x5fbb60` → `0x493910`) ends the selection with
/// `CMSG_SET_SELECTION 0` as the net stage ends, unless a party or raid member or pet keeps it as
/// a guid (`0x5fbbcd`), which the selection's tracker settles.
///
/// The model stays on the same entity, so removing its [`Guid`] is what ends the object for every
/// consumer; the [`NetEntity`] and [`ObjectStore`] stay so the model draws out (a decaying corpse
/// stays lying down). The caller has already removed the guid from the [`GuidIndex`], so a block
/// that brings the object back spawns a fresh entity beside the fading model.
fn tear_down(mut ent: EntityWorldMut) {
    ent.remove::<Guid>();
    ent.insert(DespawnFade::default());
}

/// `SMSG_MONSTER_MOVE` or `SMSG_MONSTER_MOVE_TRANSPORT`: settles transport membership, then
/// attaches the spline, or clears it and turns the unit to the facing at once. With a
/// `transport`, `start`, the path and an `Angle` facing are deck-local and the unit rides as a
/// [`TransportRider`]; without one, a riding unit leaves the deck, as vmangos drops it
/// (`MoveSplineInit.cpp:156-159`).
fn monster_move(
    guid: u64,
    transport: Option<u64>,
    start: [f32; 3],
    spline_id: u32,
    path: Vec<[f32; 3]>,
    facing: MonsterMoveFacing,
    stop: bool,
    duration_ms: u32,
    flying: bool,
    run_mode: bool,
    rooted: bool,
    // The display smoother turns this unit (`facing::smoothed`).
    smooth: bool,
    commands: &mut Commands,
    index: &GuidIndex,
    transforms: &mut Query<&mut Transform>,
    riders: &mut Query<&mut crate::transport::TransportRider>,
) {
    if let Some(&e) = index.0.get(&guid) {
        // The desync readout: the gap between where we draw the unit and where its path begins.
        trace_move_snap(
            guid,
            // Both sides in one frame: a deck packet's `start` is compared to the local pose.
            match transport {
                Some(_) => riders.get(e).ok().map(|r| r.local_pos),
                None => transforms.get(e).ok().map(|t| bevy_to_wow(t.translation)),
            },
            start,
            stop,
            duration_ms,
        );
        // A rooted unit takes no path: `0x6187a0`, the only route into the spline installer
        // `0x7c6a50`, returns on ROOT (`0x6187c2`). Only the path is refused: the un-nock tail runs
        // on every path (`0x6020ef`), and the rest for a rooted unit is untraced.
        let spline = if rooted {
            None
        } else {
            monster_move_spline(
                path,
                spline_id,
                stop,
                duration_ms,
                flying,
                run_mode,
                facing,
                transport,
            )
        };
        // The final facing (moveType 2/3/4) turns the unit now only when no path installs
        // (`0x601fee` to `0x60205e`): an `Angle` through `0x6198e0`, a `Target` through `0x602e80`,
        // a `Spot` as `0x47f220` from the unit's position, each into the raw facing. A path keeps
        // it for its end (`0x618840`, `0x618870`, `0x6188a0`; [`Spline::final_facing`]).
        let face_now = if spline.is_none() {
            facing
        } else {
            MonsterMoveFacing::None
        };
        let target_pos = |g: u64| {
            index
                .0
                .get(&g)
                .and_then(|&te| transforms.get(te).ok())
                .map(|t| bevy_to_wow(t.translation))
        };
        // Transport membership, settled before any pose is read and whether or not the path was
        // refused: the root gate refuses only the path. An `Angle` facing is deck-local
        // (`MoveSplineInit.cpp:87`); a `Spot` or `Target` is not converted server-side, so it is
        // rebased by the deck's yaw.
        let deck_yaw = |t: u64| {
            index
                .0
                .get(&t)
                .and_then(|&te| transforms.get(te).ok())
                .map(|tf| tf.rotation.to_euler(EulerRot::YXZ).0)
        };
        // The unit's own position anchors the bearing (`0x6020a5`, `0x602ec1`, its world
        // `GetPosition`), not the wire's `start`.
        let world_pos = transforms.get(e).ok().map(|tf| bevy_to_wow(tf.translation));
        if let Some(t) = transport {
            let local_facing = match face_now {
                MonsterMoveFacing::None => None,
                MonsterMoveFacing::Angle(a) => Some(a),
                other => world_pos
                    .and_then(|p| resolve_facing(other, p, target_pos))
                    .zip(deck_yaw(t))
                    .map(|(world, yaw)| world - yaw),
            };
            match riders.get_mut(e) {
                Ok(mut rider) => {
                    rider.transport_guid = t;
                    rider.local_pos = start;
                    if let Some(o) = local_facing {
                        rider.local_orientation = o;
                    }
                }
                Err(_) => {
                    commands.entity(e).insert(crate::transport::TransportRider {
                        transport_guid: t,
                        local_pos: start,
                        local_orientation: local_facing.unwrap_or_default(),
                    });
                }
            }
        } else {
            if riders.get(e).is_ok() {
                commands
                    .entity(e)
                    .remove::<crate::transport::TransportRider>();
            }
            // Only the raw facing turns; the display follows through `0x600cd0`, never reset on
            // a move. A body the smoother does not govern shows it at once.
            if let Some(orientation) =
                world_pos.and_then(|p| resolve_facing(face_now, p, target_pos))
            {
                if smooth {
                    set_raw_facing(commands, e, orientation);
                } else if let Ok(mut t) = transforms.get_mut(e) {
                    t.rotation = Quat::from_rotation_y(orientation);
                }
            }
        }
        // Every spline move un-nocks and ends the precast here. The handler `0x6018f0` (opcodes
        // 0xDD, 0x2AE) ends through `0x60d040` with the unit's own `+0xc8c` (`0x6020e8`) and
        // `0x60f530` (un-nock) on every path but one: a stop within 3 yd of the stop point jumps
        // past both (`0x6019e4`). The end's loot release reads the flags the path just wrote: a
        // path's install starts the move (`0x7c6ad4`), a refused one stops it (`0x6187b1`).
        commands
            .entity(e)
            .remove::<crate::creature_anim::NockLatch>();
        crate::creature_anim::end_precast(
            commands,
            e,
            crate::creature_anim::PrecastTail::Moving(!rooted),
        );
        if rooted {
            return;
        }
        // The spline supersedes the relay stream: a splined player (charge, taxi) would otherwise
        // keep stale flags that outrank the spline in the anim selector.
        commands.entity(e).remove::<RemoteMotion>();
        match spline {
            Some(spline) => {
                commands.entity(e).insert(spline).remove::<SplineStopped>();
            }
            // A stop keeps the id, which the server awaits back for a player-driven unit.
            None => {
                commands
                    .entity(e)
                    .remove::<Spline>()
                    .insert(SplineStopped(spline_id));
            }
        }
    }
}

/// This unit's granted modes; no component means all bits clear.
fn modes_of(guid: u64, index: &GuidIndex, modes: &Query<&mut UnitMoveModes>) -> UnitMoveModes {
    index
        .0
        .get(&guid)
        .and_then(|&e| modes.get(e).ok().copied())
        .unwrap_or_default()
}

/// `SMSG_SPLINE_MOVE_*`: a mode on a unit we do not control, with no ack. An unknown guid is
/// dropped, as the reference's handler (`0x603c80`) does; our own guid is not special-cased, as
/// vmangos never sends it for a player's mover.
fn spline_move_mode(
    guid: u64,
    mode: SplineMode,
    apply: bool,
    commands: &mut Commands,
    index: &GuidIndex,
    modes: &mut Query<&mut UnitMoveModes>,
    remote: &mut Query<&mut RemoteMotion>,
) {
    let Some(&e) = index.0.get(&guid) else {
        return; // the reference drops it too
    };
    let mut word = modes_of(guid, index, modes);
    word.set(mode, apply);
    if let Ok(mut live) = modes.get_mut(e) {
        *live = word;
    } else {
        commands.entity(e).insert(word);
    }

    // `SetRoot`'s one-shot wipe (`0x7c7340`): it clears `0xffe07f00`'s complement and stops the
    // fall (`0x7c6290`), so the unit fails the integration gate and a relayed player stops dead.
    // A creature has no direction bits; `monster_move` refuses its path instead.
    if mode == SplineMode::Root && apply {
        if let Ok(mut rm) = remote.get_mut(e) {
            use crate::creature_anim::move_flags;
            rm.flags &= ROOT_APPLY_WIPE & !(move_flags::FALLING | move_flags::FALLING_FAR);
            rm.speed = 0.0;
            rm.vertical_velocity = 0.0;
            rm.jump_xy_vel = [0.0; 2];
            rm.fall_start = None;
        }
    }
}

/// `SMSG_GAMEOBJECT_QUERY_RESPONSE`: cached, with the lockId resolved from its type's `data[]`
/// slot for use-vs-cast.
fn gameobject_info(
    entry: u32,
    type_id: u32,
    display_id: u32,
    name: String,
    data: &[i32; 24],
    go_templates: &mut GameObjectTemplates,
) {
    debug!("net: gameobject template {entry} type {type_id} display {display_id} {name:?}");
    go_templates.insert(entry, type_id, name, data);
}

/// Writes one slot of a mover's [`UnitSpeeds`]; `false` when there is none (an untracked guid,
/// or a create with no movement block). vmangos sends a self create and a speed change back to
/// back (`HandleMoveWorldportAckOpcode`), and wire order holds as each handler applies in turn.
fn set_speed(
    guid: u64,
    kind: SpeedKind,
    speed: f32,
    index: &GuidIndex,
    speeds: &mut Query<&mut UnitSpeeds>,
) -> bool {
    let Some(mut s) = index.0.get(&guid).and_then(|&e| speeds.get_mut(e).ok()) else {
        return false;
    };
    let s = &mut s.0;
    match kind {
        SpeedKind::Walk => s.walk = speed,
        SpeedKind::Run => s.run = speed,
        SpeedKind::RunBack => s.run_back = speed,
        SpeedKind::Swim => s.swim = speed,
        SpeedKind::SwimBack => s.swim_back = speed,
        SpeedKind::TurnRate => s.turn_rate = speed,
    }
    true
}

/// `SMSG_FORCE_*_SPEED_CHANGE`: applied to the mover's speeds, and for our own guid forwarded to
/// the controller, which owes the ack. A foreign mover is never acked, which the server treats as
/// an error.
fn force_speed_change(
    guid: u64,
    kind: SpeedKind,
    counter: u32,
    speed: f32,
    index: &GuidIndex,
    speeds: &mut Query<&mut UnitSpeeds>,
    self_guid: &SelfGuid,
    speed_changes: &mut MessageWriter<SpeedChangeMessage>,
) {
    let applied = set_speed(guid, kind, speed, index, speeds);
    if self_guid.0 == Some(guid) {
        info!("net: force {kind:?} speed change -> {speed} yd/s (counter {counter})");
        // Should be unreachable; the ack still goes, so it warns.
        if !applied {
            warn!(
                "net: force {kind:?} speed change for OUR mover {guid:#x} landed nowhere \
                 (no live speed set) — we still ack, so the server and we now disagree"
            );
        }
        speed_changes.write(SpeedChangeMessage {
            guid,
            kind,
            counter,
            speed,
        });
    } else {
        debug!("net: force speed change for foreign mover {guid:#x} — applied {applied}, no ack");
    }
}

/// The SPLINE_SET and MOVE_SET families: an observed unit's speed, no ack. MOVE_SET's pose
/// arrives as its own `UnitMove`.
fn speed_changed(
    guid: u64,
    kind: SpeedKind,
    speed: f32,
    index: &GuidIndex,
    speeds: &mut Query<&mut UnitSpeeds>,
) {
    if !set_speed(guid, kind, speed, index, speeds) {
        // A speed broadcast can outrun the create block, which carries the same set.
        debug!("net: {kind:?} speed change for untracked mover {guid:#x} — nothing to write");
    }
}

/// One update packet's events, as the protocol decodes them: its blocks, then its end.
#[cfg(test)]
pub(crate) fn packet(blocks: impl IntoIterator<Item = SessionEvent>) -> Vec<SessionEvent> {
    blocks
        .into_iter()
        .chain([SessionEvent::ObjectUpdateEnd])
        .collect()
}

/// `events` as the wire brings them one packet each: every update block closed as its own packet,
/// any other event as it is.
#[cfg(test)]
pub(crate) fn wire(events: impl IntoIterator<Item = SessionEvent>) -> Vec<SessionEvent> {
    events
        .into_iter()
        .flat_map(|ev| {
            let block = matches!(
                ev,
                SessionEvent::ObjectCreate { .. }
                    | SessionEvent::ItemCreate { .. }
                    | SessionEvent::ObjectValues { .. }
                    | SessionEvent::ObjectMove { .. }
                    | SessionEvent::ObjectsRemoved(_)
                    | SessionEvent::ObjectsNear(_)
                    | SessionEvent::ObjectNotify { .. }
            );
            std::iter::once(ev).chain(block.then_some(SessionEvent::ObjectUpdateEnd))
        })
        .collect()
}

#[cfg(test)]
mod update_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::ClientCommand;
    use benilla_protocol::guid;

    /// The object packets run the item watchers in wire order: the create hook, a values block,
    /// a seat of ours, a late record, the destroy; another player's item and a resend run none.
    #[test]
    fn the_object_packets_write_the_item_watchers_notices_in_wire_order() {
        use crate::item_watch::ItemNotice;
        use benilla_protocol::field::{
            FIELD_ITEM_STACK_COUNT, FIELD_OBJECT_ENTRY, FIELD_PLAYER_INV_SLOT_HEAD,
        };
        use benilla_protocol::messages::ObjectType;
        const ME: u64 = 0x0000_0000_0000_0007;
        const POTIONS: u64 = 0x4000_0000_0000_0042;
        const SCROLL: u64 = 0x4000_0000_0000_0043;
        const THEIRS: u64 = 0x4000_0000_0000_0044;
        const POTION: u32 = 118;
        const SCROLL_ID: u32 = 955;
        // `OBJECT_FIELD_TYPE` item, the entry, `ITEM_FIELD_OWNER` and a stack of one.
        let create = |guid: u64, entry: u32, owner: u64| SessionEvent::ItemCreate {
            guid,
            container: false,
            fields: ObjectFields::from_pairs(&[
                (2, 0x3),
                (FIELD_OBJECT_ENTRY, entry),
                (6, owner as u32),
                (7, 0),
                (FIELD_ITEM_STACK_COUNT, 1),
            ])
            .into_created(ObjectType::Item),
        };
        let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
        crate::lanes::tap_tests::tap(&mut app);
        let world = app.world_mut();
        let me = world
            .spawn((
                Guid(ME),
                ObjectStore(ObjectFields::default().into_created(ObjectType::Player)),
            ))
            .id();
        world.resource_mut::<GuidIndex>().0.insert(ME, me);
        world.resource_mut::<SelfGuid>().0 = Some(ME);
        world
            .resource_mut::<Items>()
            .insert_template(POTION, Some(crate::items::test_template("Potion")));
        let pack_1 = FIELD_PLAYER_INV_SLOT_HEAD + 2 * 23;
        super::super::handlers::dispatch(
            world,
            super::wire(vec![
                create(POTIONS, POTION, ME),
                create(SCROLL, SCROLL_ID, ME),
                create(THEIRS, POTION, 0x8),
                SessionEvent::ObjectValues {
                    guid: POTIONS,
                    fields: ObjectFields::from_pairs(&[(FIELD_ITEM_STACK_COUNT, 3)]),
                },
                SessionEvent::ObjectValues {
                    guid: POTIONS,
                    fields: ObjectFields::from_pairs(&[(FIELD_ITEM_STACK_COUNT, 3)]),
                },
                SessionEvent::ObjectValues {
                    guid: THEIRS,
                    fields: ObjectFields::from_pairs(&[(FIELD_ITEM_STACK_COUNT, 3)]),
                },
                SessionEvent::ObjectValues {
                    guid: ME,
                    fields: ObjectFields::from_pairs(&[
                        (pack_1, POTIONS as u32),
                        (pack_1 + 1, (POTIONS >> 32) as u32),
                    ]),
                },
                SessionEvent::ItemTemplate {
                    entry: SCROLL_ID,
                    info: Some(Box::new(crate::items::test_template("Scroll"))),
                },
                SessionEvent::ObjectDestroyed(POTIONS),
                SessionEvent::ObjectDestroyed(THEIRS),
            ]),
        );
        let notices: Vec<ItemNotice> = world
            .resource_mut::<crate::lanes::tap_tests::Tapped>()
            .item_notices();
        assert_eq!(
            notices,
            [
                ItemNotice::Record {
                    guid: POTIONS,
                    entry: POTION
                },
                ItemNotice::StackCount {
                    guid: POTIONS,
                    entry: POTION
                },
                ItemNotice::Seat {
                    old: None,
                    new: Some(POTION)
                },
                ItemNotice::Record {
                    guid: SCROLL,
                    entry: SCROLL_ID
                },
                ItemNotice::Destroyed {
                    guid: POTIONS,
                    entry: POTION
                },
            ]
        );
    }

    /// `0x4c0340`: every trade getter call that missed the cache registered its own callback, and
    /// the record's answer runs each once, in call order, signalling for a ready record only
    /// (`0x4c0343`); a refusal frees them silent.
    #[test]
    fn an_item_answer_runs_each_waiting_trade_getters_callback_once() {
        use crate::item_watch::ItemNotice;
        let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
        crate::lanes::tap_tests::tap(&mut app);
        let world = app.world_mut();
        {
            let items = world.resource::<Items>();
            for entry in [4306, 4306, 2592, 7777] {
                items.wait_for_trade_record(entry);
            }
        }
        let answer = |entry: u32, ok: bool| SessionEvent::ItemTemplate {
            entry,
            info: ok.then(|| Box::new(crate::items::test_template("Thing"))),
        };
        super::super::handlers::dispatch(
            world,
            super::wire(vec![answer(4306, true), answer(7777, false)]),
        );
        let notices: Vec<ItemNotice> = world
            .resource_mut::<crate::lanes::tap_tests::Tapped>()
            .item_notices();
        assert_eq!(
            notices,
            [
                ItemNotice::TradeRecord { entry: 4306 },
                ItemNotice::TradeRecord { entry: 4306 }
            ],
            "one per call that asked, none for the refusal"
        );
        // The callbacks are spent: a second answer, or the refused entry's later one, runs none.
        super::super::handlers::dispatch(
            world,
            super::wire(vec![answer(4306, true), answer(7777, true)]),
        );
        assert!(world
            .resource_mut::<crate::lanes::tap_tests::Tapped>()
            .item_notices()
            .is_empty());
        // The one still waiting runs on its own answer.
        super::super::handlers::dispatch(world, super::wire(vec![answer(2592, true)]));
        assert_eq!(
            world
                .resource_mut::<crate::lanes::tap_tests::Tapped>()
                .item_notices(),
            [ItemNotice::TradeRecord { entry: 2592 }]
        );
    }

    /// The item paths' `UNIT_INVENTORY_CHANGED`, in wire order: a create's record set-up for its
    /// owner, whoever's (`0x5d85fe`); a worn item's restack (`0x5d950e` → `0x4c72e0`); the late
    /// record of an item of ours; the worn item's destroy (`0x5d87c1`). A backpack restack is none.
    #[test]
    fn the_object_packets_write_the_item_paths_inventory_signals_in_wire_order() {
        use crate::field_watch::FieldNotice;
        use benilla_protocol::field::{
            FIELD_ITEM_STACK_COUNT, FIELD_OBJECT_ENTRY, FIELD_PLAYER_INV_SLOT_HEAD,
        };
        use benilla_protocol::messages::ObjectType;
        const ME: u64 = 0x0000_0000_0000_0007;
        const HELM: u64 = 0x4000_0000_0000_0042;
        const POTIONS: u64 = 0x4000_0000_0000_0043;
        const THEIRS: u64 = 0x4000_0000_0000_0044;
        const HELM_ID: u32 = 7366;
        const POTION: u32 = 118;
        let create = |guid: u64, entry: u32, owner: u64| SessionEvent::ItemCreate {
            guid,
            container: false,
            fields: ObjectFields::from_pairs(&[
                (2, 0x3),
                (FIELD_OBJECT_ENTRY, entry),
                (6, owner as u32),
                (7, 0),
                (8, owner as u32),
                (9, 0),
                (FIELD_ITEM_STACK_COUNT, 1),
            ])
            .into_created(ObjectType::Item),
        };
        let seat = |n: u16, guid: u64| {
            let lo = FIELD_PLAYER_INV_SLOT_HEAD + 2 * n;
            [(lo, guid as u32), (lo + 1, (guid >> 32) as u32)]
        };
        let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
        crate::lanes::tap_tests::tap(&mut app);
        let world = app.world_mut();
        let pairs: Vec<(u16, u32)> = seat(0, HELM).into_iter().chain(seat(23, POTIONS)).collect();
        let me = world
            .spawn((
                Guid(ME),
                ObjectStore(ObjectFields::from_pairs(&pairs).into_created(ObjectType::Player)),
            ))
            .id();
        world.resource_mut::<GuidIndex>().0.insert(ME, me);
        world.resource_mut::<SelfGuid>().0 = Some(ME);
        world
            .resource_mut::<Items>()
            .insert_template(POTION, Some(crate::items::test_template("Potion")));
        let restack = |guid: u64| SessionEvent::ObjectValues {
            guid,
            fields: ObjectFields::from_pairs(&[(FIELD_ITEM_STACK_COUNT, 2)]),
        };
        super::super::handlers::dispatch(
            world,
            super::wire(vec![
                create(POTIONS, POTION, ME),
                create(HELM, HELM_ID, ME),
                create(THEIRS, POTION, 0x8),
                restack(POTIONS),
                restack(HELM),
                SessionEvent::ItemTemplate {
                    entry: HELM_ID,
                    info: Some(Box::new(crate::items::test_template("Helm"))),
                },
                SessionEvent::ObjectDestroyed(HELM),
            ]),
        );
        // Both lanes, in stamp order: the item watchers' runs carry their own signals.
        let mut stamped: Vec<(crate::lanes::Stamp, FieldNotice)> = world
            .resource_mut::<crate::lanes::tap_tests::Tapped>()
            .fields_stamped();
        stamped.extend(
            world
                .resource_mut::<crate::lanes::tap_tests::Tapped>()
                .items_stamped()
                .into_iter()
                .flat_map(|(s, r)| r.signals.into_iter().map(move |n| (s, n))),
        );
        stamped.sort_by_key(|&(s, _)| s);
        let inventory: Vec<FieldNotice> = stamped
            .into_iter()
            .map(|(_, n)| n)
            .filter(|n| match *n {
                FieldNotice::Unit { id, .. }
                | FieldNotice::Signal { id, .. }
                | FieldNotice::Player { id } => {
                    id == crate::field_watch::id::UNIT_INVENTORY_CHANGED
                }
                _ => false,
            })
            .collect();
        let owner = |guid| FieldNotice::Signal {
            guid,
            id: crate::field_watch::id::UNIT_INVENTORY_CHANGED,
        };
        let slot = FieldNotice::Player {
            id: crate::field_watch::id::UNIT_INVENTORY_CHANGED,
        };
        assert_eq!(
            inventory,
            [owner(ME), owner(0x8), slot, owner(ME), slot],
            "the potions' set-up, theirs, the helm's restack, its late record, its destroy"
        );
    }

    /// The ammunition getters' record callback `0x4c84f0`: each call that missed runs once on a
    /// ready answer, `UNIT_INVENTORY_CHANGED` with `"player"` (`0x4c8509`); a refusal frees them.
    #[test]
    fn an_item_answer_runs_each_waiting_ammo_getters_callback_once() {
        use crate::field_watch::FieldNotice;
        let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
        crate::lanes::tap_tests::tap(&mut app);
        let world = app.world_mut();
        {
            let items = world.resource::<Items>();
            for entry in [2512, 2512, 3030] {
                items.wait_for_ammo_record(entry);
            }
        }
        let answer = |entry: u32, ok: bool| SessionEvent::ItemTemplate {
            entry,
            info: ok.then(|| Box::new(crate::items::test_template("Arrow"))),
        };
        super::super::handlers::dispatch(
            world,
            super::wire(vec![
                answer(2512, true),
                answer(3030, false),
                answer(3030, true),
            ]),
        );
        let notices: Vec<FieldNotice> = world
            .resource_mut::<crate::lanes::tap_tests::Tapped>()
            .fields();
        assert_eq!(
            notices,
            [FieldNotice::Player {
                id: crate::field_watch::id::UNIT_INVENTORY_CHANGED
            }; 2],
            "one per call that asked, none for the refusal or after it"
        );
    }

    /// The object packets run the field watches per block in wire order: our first create none
    /// (`0x465d80`), a values block each watch it moved once, a create of an object already shown
    /// each watch it moved over every field (`0x465d8e`); another unit only its bridge.
    #[test]
    fn the_object_packets_write_the_field_watches_notices_per_block() {
        use crate::field_watch::FieldNotice;
        use benilla_protocol::messages::ObjectType;
        const ME: u64 = 0x0000_0000_0000_0007;
        const MOB: u64 = 0xF130_0000_0000_0001;
        const XP: u16 = 716;
        const STAT0: u16 = 150;
        let create = |guid: u64, kind: EntityKind, pairs: &[(u16, u32)]| {
            let created = if kind == EntityKind::Player {
                ObjectType::Player
            } else {
                ObjectType::Unit
            };
            SessionEvent::ObjectCreate {
                guid,
                kind,
                display_id: None,
                position: [0.0; 3],
                orientation: 0.0,
                scale: 1.0,
                speeds: None,
                mover: None,
                transport_progress: None,
                transport: None,
                spline: None,
                fields: ObjectFields::from_pairs(pairs).into_created(created),
            }
        };
        let values = |guid: u64, pairs: &[(u16, u32)]| SessionEvent::ObjectValues {
            guid,
            fields: ObjectFields::from_pairs(pairs),
        };
        let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
        crate::lanes::tap_tests::tap(&mut app);
        let world = app.world_mut();
        world.resource_mut::<SelfGuid>().0 = Some(ME);
        let drain = |world: &mut World| -> Vec<FieldNotice> {
            world
                .resource_mut::<crate::lanes::tap_tests::Tapped>()
                .fields()
        };
        super::super::handlers::dispatch(
            world,
            super::wire(vec![
                create(ME, EntityKind::Player, &[(STAT0, 20), (XP, 10)]),
                create(MOB, EntityKind::Unit, &[(STAT0, 5)]),
            ]),
        );
        assert_eq!(
            drain(world),
            [
                &[crate::field_watch::FieldNotice::TokenFlags {
                    guid: ME,
                    change: benilla_ui::script::TokenChange::Update,
                }],
                &crate::field_watch::guild_tag(ME)[..],
                &[crate::field_watch::FieldNotice::RecruitmentCascade],
                &[crate::field_watch::FieldNotice::TokenFlags {
                    guid: MOB,
                    change: benilla_ui::script::TokenChange::Update,
                }],
            ]
            .concat(),
            "a first create runs no notify pass, only each unit's `PostInit`"
        );
        super::super::handlers::dispatch(
            world,
            super::wire(vec![
                values(ME, &[(XP, 30)]),
                values(MOB, &[(STAT0, 6), (STAT0 + 1, 2)]),
                // Two blocks on one watch: two runs.
                values(ME, &[(XP, 40)]),
                values(ME, &[(STAT0, 21), (XP, 40)]),
                // A create of the shown player runs each watch it moved, over every field.
                create(ME, EntityKind::Player, &[(STAT0, 21), (XP, 50)]),
            ]),
        );
        assert_eq!(
            drain(world),
            [
                FieldNotice::Player { id: 189 },
                FieldNotice::Unit { guid: MOB, id: 144 },
                FieldNotice::Unit { guid: MOB, id: 145 },
                FieldNotice::Player { id: 189 },
                FieldNotice::Unit { guid: ME, id: 144 },
                FieldNotice::Player { id: 189 },
            ]
        );
    }

    /// A player's first create runs its `PostInit` (`0x5debe0`): its guild tag (`0x5dec19`), then,
    /// for another player, its party record against its descriptor, the difference signalled and
    /// nothing written back (`0x5dec5f`). A create of a live player is neither a first create nor
    /// a reactivation, so it runs neither; our own create compares nothing.
    #[test]
    fn another_players_create_signals_its_record_against_its_descriptor() {
        use crate::field_watch::FieldNotice;
        use benilla_protocol::messages::{ObjectType, PartyMemberStatsInfo};
        const ME: u64 = 0x07;
        const MATE: u64 = 0x2001;
        const HEALTH: u16 = 22;
        const MAXHEALTH: u16 = 28;
        const LEVEL: u16 = 34;
        let create = |guid: u64, pairs: &[(u16, u32)]| SessionEvent::ObjectCreate {
            guid,
            kind: EntityKind::Player,
            display_id: None,
            position: [0.0; 3],
            orientation: 0.0,
            scale: 1.0,
            speeds: None,
            mover: None,
            transport_progress: None,
            transport: None,
            spline: None,
            fields: ObjectFields::from_pairs(pairs).into_created(ObjectType::Player),
        };
        let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
        crate::lanes::tap_tests::tap(&mut app);
        let world = app.world_mut();
        world.resource_mut::<SelfGuid>().0 = Some(ME);
        let record = PartyMemberStatsInfo {
            status: Some(1),
            cur_hp: Some(100),
            max_hp: Some(100),
            power_type: Some(0),
            cur_power: Some(0),
            max_power: Some(0),
            level: Some(60),
            auras: Some(Vec::new()),
            auras_negative: Some(Vec::new()),
            ..Default::default()
        };
        world
            .resource_mut::<crate::ui_party::GroupState>()
            .stats
            .insert(MATE, record.clone());
        let drain = |world: &mut World| -> Vec<FieldNotice> {
            world
                .resource_mut::<crate::lanes::tap_tests::Tapped>()
                .fields()
        };
        let mate = [(HEALTH, 100), (MAXHEALTH, 200), (LEVEL, 60)];
        super::super::handlers::dispatch(world, super::wire(vec![create(MATE, &mate)]));
        let signal = |id| FieldNotice::Signal { guid: MATE, id };
        let gate = FieldNotice::Plate {
            guid: MATE,
            call: crate::vplates::PlateCall::Gate,
        };
        assert_eq!(
            drain(world),
            [
                FieldNotice::TokenFlags {
                    guid: MATE,
                    change: benilla_ui::script::TokenChange::Update,
                },
                gate,
                FieldNotice::Global { id: 376 },
                signal(345),
                signal(22)
            ],
            "its `PostInit`'s token flags, the guild tag, its plate's gate first, then the maximum \
             differs"
        );
        super::super::handlers::dispatch(world, super::wire(vec![create(MATE, &mate)]));
        assert_eq!(
            drain(world),
            [],
            "a live re-create: no hook, no `PostUpdate`"
        );
        assert_eq!(
            world.resource::<crate::ui_party::GroupState>().stats[&MATE],
            record
        );
        super::super::handlers::dispatch(world, super::wire(vec![create(ME, &mate)]));
        assert_eq!(
            drain(world),
            [
                &[FieldNotice::TokenFlags {
                    guid: ME,
                    change: benilla_ui::script::TokenChange::Update,
                }],
                &crate::field_watch::guild_tag(ME)[..],
                &[FieldNotice::RecruitmentCascade],
            ]
            .concat(),
            "our own create: its token flags, the guild tag, then the recruitment cascade \
             (`0x5dec1e`)"
        );
    }

    /// The object handlers over a headless client with our player live (an empty store created
    /// as a player), the potion's template cached; the items it lists created, our own.
    fn watch_world(items: &[(u64, u32, bool)]) -> App {
        use benilla_protocol::messages::ObjectType;
        let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
        crate::lanes::tap_tests::tap(&mut app);
        let world = app.world_mut();
        let me = world
            .spawn((
                Guid(WATCH_ME),
                ObjectStore(ObjectFields::default().into_created(ObjectType::Player)),
            ))
            .id();
        world.resource_mut::<GuidIndex>().0.insert(WATCH_ME, me);
        world.resource_mut::<SelfGuid>().0 = Some(WATCH_ME);
        world
            .resource_mut::<Items>()
            .insert_template(118, Some(crate::items::test_template("Potion")));
        let creates = items
            .iter()
            .map(|&(guid, entry, container)| watched_item(guid, entry, container));
        super::super::handlers::dispatch(world, super::wire(creates));
        drain_notices(world);
        app
    }

    const WATCH_ME: u64 = 0x0000_0000_0000_0007;

    /// Our item's create block: type item (or container, 4 seats), the entry, ourselves as owner
    /// and a stack of one.
    fn watched_item(guid: u64, entry: u32, container: bool) -> SessionEvent {
        use benilla_protocol::field::{FIELD_ITEM_STACK_COUNT, FIELD_OBJECT_ENTRY};
        use benilla_protocol::messages::ObjectType;
        let mut pairs = vec![
            (2, if container { 0x7 } else { 0x3 }),
            (FIELD_OBJECT_ENTRY, entry),
            (6, WATCH_ME as u32),
            (7, 0),
            (FIELD_ITEM_STACK_COUNT, 1),
        ];
        if container {
            pairs.push((48, 4));
        }
        SessionEvent::ItemCreate {
            guid,
            container,
            fields: ObjectFields::from_pairs(&pairs).into_created(if container {
                ObjectType::Container
            } else {
                ObjectType::Item
            }),
        }
    }

    fn drain_notices(world: &mut World) -> Vec<crate::item_watch::ItemNotice> {
        world
            .resource_mut::<crate::lanes::tap_tests::Tapped>()
            .item_notices()
    }

    /// Every item's first create asks for its record with the callback `0x5d8390` (`0x5d8300`),
    /// and so does an entry change to an uncached entry (`0x5d9652`); a re-create and a zero
    /// entry ask nothing. A refused record is asked again by the next create or entry change.
    #[test]
    fn an_items_record_is_asked_at_its_create_and_entry_change_only() {
        use benilla_protocol::field::FIELD_OBJECT_ENTRY;
        const A: u64 = 0x4000_0000_0000_00a1;
        const B: u64 = 0x4000_0000_0000_00b2;
        let mut app = watch_world(&[]);
        let (tx, rx) = crossbeam_channel::unbounded();
        app.world_mut().insert_resource(NetCommands(tx));
        let asked = |rx: &crossbeam_channel::Receiver<ClientCommand>| -> Vec<u32> {
            crate::query_cache::queries(rx)
                .into_iter()
                .filter_map(|c| match c {
                    ClientCommand::ItemQuery { entry, .. } => Some(entry),
                    _ => None,
                })
                .collect()
        };
        let dispatch = |app: &mut App, ev: SessionEvent| {
            super::super::handlers::dispatch(app.world_mut(), super::wire(vec![ev]));
        };
        dispatch(&mut app, watched_item(A, 955, false));
        assert_eq!(asked(&rx), [955], "the create asks");
        dispatch(
            &mut app,
            SessionEvent::ItemTemplate {
                entry: 955,
                info: None,
            },
        );
        dispatch(&mut app, watched_item(A, 955, false));
        assert!(asked(&rx).is_empty(), "a re-create runs no set-up");
        dispatch(&mut app, watched_item(B, 955, false));
        assert_eq!(asked(&rx), [955], "the next create asks again");
        dispatch(
            &mut app,
            SessionEvent::ObjectValues {
                guid: A,
                fields: ObjectFields::from_pairs(&[(FIELD_OBJECT_ENTRY, 956)]),
            },
        );
        assert_eq!(asked(&rx), [956], "an entry change to a miss asks");
        dispatch(&mut app, watched_item(0x4000_0000_0000_00c3, 0, false));
        assert!(asked(&rx).is_empty(), "entry 0 has no key (`0x55ba42`)");
    }

    /// An entry change to an uncached entry waits on the answer with its own callback
    /// (`0x5d96b0`), not the create's set-up; a negative answer runs nothing.
    #[test]
    fn an_entry_change_that_misses_the_cache_runs_its_callback_on_the_answer() {
        use crate::item_watch::ItemNotice;
        use benilla_protocol::field::FIELD_OBJECT_ENTRY;
        const POTIONS: u64 = 0x4000_0000_0000_0042;
        let mut app = watch_world(&[(POTIONS, 118, false)]);
        let world = app.world_mut();
        let to = |entry: u32| SessionEvent::ObjectValues {
            guid: POTIONS,
            fields: ObjectFields::from_pairs(&[(FIELD_OBJECT_ENTRY, entry)]),
        };
        let answer = |entry: u32, known: bool| SessionEvent::ItemTemplate {
            entry,
            info: known.then(|| Box::new(crate::items::test_template("Scroll"))),
        };
        super::super::handlers::dispatch(world, super::wire(vec![to(955), answer(955, true)]));
        assert_eq!(
            drain_notices(world),
            [
                ItemNotice::Entry {
                    guid: POTIONS,
                    old: 118,
                    new: 955,
                    new_cached: false
                },
                ItemNotice::EntryRecord {
                    guid: POTIONS,
                    entry: 955
                },
            ]
        );
        super::super::handlers::dispatch(world, super::wire(vec![to(956), answer(956, false)]));
        assert_eq!(
            drain_notices(world),
            [ItemNotice::Entry {
                guid: POTIONS,
                old: 955,
                new: 956,
                new_cached: false
            }],
            "a negative answer runs nothing"
        );
    }

    /// `0x5588e0`'s refusal leg: a game object entry the server refuses is no template, where a
    /// zeroed one stood in for it. It reads unknown, a hover does not ask it again, and the next
    /// spawn of the entry does.
    #[test]
    fn a_refused_game_object_entry_is_no_template() {
        const ENTRY: u32 = 1731;
        let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
        crate::lanes::tap_tests::tap(&mut app);
        let world = app.world_mut();
        let fields = crate::go_templates::test_fields(ENTRY);
        let (tx, rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        world
            .resource::<GameObjectTemplates>()
            .request_at_create(0x11, &fields, &commands);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::GameObjectQuery { entry: ENTRY, .. })
        ));

        super::super::handlers::dispatch(
            world,
            super::wire(vec![SessionEvent::GameObjectUnknown { entry: ENTRY }]),
        );
        let templates = world.resource::<GameObjectTemplates>();
        assert!(
            templates.of(&fields).is_none(),
            "no zeroed template stands in"
        );
        assert!(templates.is_unknown(ENTRY));
        assert!(rx.try_recv().is_err(), "reading it asks nothing");
        // A hover reads the record every frame and never asks: the builder (`0x52aa20`) calls no
        // object cache lookup.
        for _ in 0..600 {
            assert!(templates.of(&fields).is_none());
        }
        assert!(
            rx.try_recv().is_err(),
            "a hovered refused object is not asked again"
        );

        templates.request_at_create(0x12, &fields, &commands);
        assert!(
            matches!(
                rx.try_recv(),
                Ok(ClientCommand::GameObjectQuery {
                    entry: ENTRY,
                    guid: 0x12
                })
            ),
            "the next spawn asks at once"
        );
    }

    /// A refused creature with a unit of it in view: the next create block of a unit of that
    /// entry asks at once (`0x60b15b`), with no window between.
    #[test]
    fn a_refused_creature_is_asked_again_when_a_new_unit_of_it_is_created() {
        const ENTRY: u32 = 1234;
        let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
        crate::lanes::tap_tests::tap(&mut app);
        let (tx, rx) = crossbeam_channel::unbounded();
        app.world_mut().insert_resource(NetCommands(tx));
        let create = |guid: u64| SessionEvent::ObjectCreate {
            guid,
            kind: EntityKind::Unit,
            display_id: None,
            position: [0.0; 3],
            orientation: 0.0,
            scale: 1.0,
            speeds: None,
            mover: None,
            transport_progress: None,
            transport: None,
            spline: None,
            fields: ObjectFields::from_pairs(&[(2, 9), (3, ENTRY)])
                .into_created(benilla_protocol::messages::ObjectType::Unit),
        };
        let creature_asks = |rx: &crossbeam_channel::Receiver<ClientCommand>| {
            rx.try_iter()
                .filter(|c| matches!(c, ClientCommand::CreatureQuery { entry: ENTRY, .. }))
                .count()
        };
        let unit =
            |counter: u64| counter | (u64::from(ENTRY) << 24) | (u64::from(guid::HIGH_UNIT) << 48);
        let (a, b) = (unit(1), unit(2));

        super::super::handlers::dispatch(app.world_mut(), super::wire(vec![create(a)]));
        assert_eq!(creature_asks(&rx), 1, "the create asks");
        app.world_mut()
            .resource_mut::<NameCache>()
            .insert_creature(ENTRY, None);
        assert_eq!(creature_asks(&rx), 0, "the refusal itself asks nothing");

        super::super::handlers::dispatch(app.world_mut(), super::wire(vec![create(b)]));
        assert_eq!(creature_asks(&rx), 1, "a new unit of it asks again");
    }

    /// `0x604600`, the entry watcher, looks the new entry up with a callback (`0x60463f`): a
    /// values block that moves a unit's entry to one not cached asks for it.
    #[test]
    fn an_entry_change_asks_the_creature_cache_for_the_new_entry() {
        const OLD: u32 = 299;
        const NEW: u32 = 2_671;
        let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
        crate::lanes::tap_tests::tap(&mut app);
        let (tx, rx) = crossbeam_channel::unbounded();
        app.world_mut().insert_resource(NetCommands(tx));
        let mob = (u64::from(OLD) << 24) | (u64::from(guid::HIGH_UNIT) << 48) | 1;
        super::super::handlers::dispatch(
            app.world_mut(),
            super::wire(vec![SessionEvent::ObjectCreate {
                guid: mob,
                kind: EntityKind::Unit,
                display_id: None,
                position: [0.0; 3],
                orientation: 0.0,
                scale: 1.0,
                speeds: None,
                mover: None,
                transport_progress: None,
                transport: None,
                spline: None,
                fields: ObjectFields::from_pairs(&[(3, OLD)])
                    .into_created(benilla_protocol::messages::ObjectType::Unit),
            }]),
        );
        let _ = rx.try_iter().count();
        super::super::handlers::dispatch(
            app.world_mut(),
            super::wire(vec![SessionEvent::ObjectValues {
                guid: mob,
                fields: ObjectFields::from_pairs(&[(3, NEW)]),
            }]),
        );
        assert!(
            rx.try_iter().any(
                |c| matches!(c, ClientCommand::CreatureQuery { entry: NEW, guid } if guid == mob)
            ),
            "the new entry is asked for"
        );
    }

    /// A unit of `entry` and a game object of `go_entry`, created as their blocks bring them.
    fn record_creates(unit: u64, entry: u32, go: u64, go_entry: u32) -> Vec<SessionEvent> {
        use benilla_protocol::messages::ObjectType;
        let create = |guid, kind, fields: ObjectFields| SessionEvent::ObjectCreate {
            guid,
            kind,
            display_id: None,
            position: [0.0; 3],
            orientation: 0.0,
            scale: 1.0,
            speeds: None,
            mover: None,
            transport_progress: None,
            transport: None,
            spline: None,
            fields,
        };
        vec![
            create(
                unit,
                EntityKind::Unit,
                ObjectFields::from_pairs(&[(2, 9), (3, entry)]).into_created(ObjectType::Unit),
            ),
            create(
                go,
                EntityKind::GameObject,
                ObjectFields::from_pairs(&[(3, go_entry)]).into_created(ObjectType::GameObject),
            ),
        ]
    }

    /// The creature and template asks the wire saw, as `(creature, game object)` counts.
    fn record_asks(rx: &crossbeam_channel::Receiver<ClientCommand>) -> (usize, usize) {
        rx.try_iter().fold((0, 0), |(c, g), cmd| match cmd {
            ClientCommand::CreatureQuery { .. } => (c + 1, g),
            ClientCommand::GameObjectQuery { .. } => (c, g + 1),
            _ => (c, g),
        })
    }

    /// The record asks belong to the create hooks (`0x5fb880` → `0x60afb0`, the ask at
    /// `0x60b15b`; `0x5f7c40`, `0x5f7d81`), which a create of an object already shown skips for
    /// the all-fields watch (`0x465d79`, `0x465d8e`): a refused entry is not asked again by it.
    #[test]
    fn a_re_create_of_a_shown_object_asks_no_record() {
        const ENTRY: u32 = 1234;
        const GO_ENTRY: u32 = 1000;
        const GO: u64 = 0xF110_0000_0000_0009;
        let mob = (u64::from(ENTRY) << 24) | (u64::from(guid::HIGH_UNIT) << 48) | 1;
        let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
        crate::lanes::tap_tests::tap(&mut app);
        let (tx, rx) = crossbeam_channel::unbounded();
        app.world_mut().insert_resource(NetCommands(tx));

        let creates = record_creates(mob, ENTRY, GO, GO_ENTRY);
        super::super::handlers::dispatch(app.world_mut(), super::wire(creates.clone()));
        assert_eq!(record_asks(&rx), (1, 1), "the first creates' hooks ask");
        let world = app.world_mut();
        world
            .resource_mut::<NameCache>()
            .insert_creature(ENTRY, None);
        world.resource_mut::<GameObjectTemplates>().refuse(GO_ENTRY);

        super::super::handlers::dispatch(app.world_mut(), super::wire(creates));
        assert_eq!(record_asks(&rx), (0, 0), "shown: no hook, no ask");
    }

    /// A game object brought back asks its record when it has none (`0x5f7ed0`: `+0x214` clear,
    /// `0x558560` at `0x5f7f31`); a unit brought back with its display unchanged asks nothing, its
    /// slot 3's display rebuild finding the same records (`0x5fbe28` → `0x60abe0` → `0x60ae10`).
    #[test]
    fn a_released_game_object_brought_back_asks_its_missing_record() {
        const ENTRY: u32 = 1234;
        const GO_ENTRY: u32 = 1000;
        const GO: u64 = 0xF110_0000_0000_0009;
        let mob = (u64::from(ENTRY) << 24) | (u64::from(guid::HIGH_UNIT) << 48) | 1;
        let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
        crate::lanes::tap_tests::tap(&mut app);
        let (tx, rx) = crossbeam_channel::unbounded();
        app.world_mut().insert_resource(NetCommands(tx));

        let creates = record_creates(mob, ENTRY, GO, GO_ENTRY);
        super::super::handlers::dispatch(app.world_mut(), super::wire(creates.clone()));
        let world = app.world_mut();
        world
            .resource_mut::<NameCache>()
            .insert_creature(ENTRY, None);
        world.resource_mut::<GameObjectTemplates>().refuse(GO_ENTRY);
        super::super::handlers::dispatch(
            app.world_mut(),
            super::wire(vec![SessionEvent::ObjectsRemoved(vec![mob, GO])]),
        );
        let _ = record_asks(&rx);

        super::super::handlers::dispatch(app.world_mut(), super::wire(creates));
        assert_eq!(record_asks(&rx), (0, 1), "the game object's slot 3 asks");
    }

    /// The display tables of the display tests: 10 and 12 share model 1, whose sound is no row,
    /// so they name the same records; 11 is model 2, with a sound row.
    fn display_tables() -> crate::entities::Creatures {
        crate::entities::Creatures::from_catalog(
            benilla_formats::CreatureCatalog::from_record_rows(
                &[(10, 1, 0, 0), (11, 2, 0, 0), (12, 1, 0, 0)],
                &[(1, 0), (2, 50)],
                &[50],
            ),
        )
    }

    /// A unit of `ENTRY` at `display`, alive, as `type_mask` (9 a creature, `0x19` a player).
    fn displayed(guid: u64, kind: EntityKind, type_mask: u32, display: u32) -> SessionEvent {
        use benilla_protocol::field::{FIELD_UNIT_DISPLAYID, FIELD_UNIT_HEALTH};
        use benilla_protocol::messages::ObjectType;
        let as_type = match kind {
            EntityKind::Player => ObjectType::Player,
            _ => ObjectType::Unit,
        };
        SessionEvent::ObjectCreate {
            guid,
            kind,
            display_id: Some(display),
            position: [0.0; 3],
            orientation: 0.0,
            scale: 1.0,
            speeds: None,
            mover: None,
            transport_progress: None,
            transport: None,
            spline: None,
            fields: ObjectFields::from_pairs(&[
                (2, type_mask),
                (3, 1234),
                (FIELD_UNIT_HEALTH, 100),
                (FIELD_UNIT_DISPLAYID, display),
            ])
            .into_created(as_type),
        }
    }

    /// The display rebuild (`0x60abe0`) is announced at its watcher's place among the update's
    /// edges, the fields written first and the watchers run after in field order: behind the
    /// level's (34), at the display's (131, `0x6049b5`), ahead of the dynamic flags' (143). A
    /// display naming the held row's records rebuilds nothing (`0x60ae10`).
    #[test]
    fn a_display_rebuild_is_announced_at_its_watchers_place() {
        use benilla_protocol::field::{
            FIELD_UNIT_DISPLAYID, FIELD_UNIT_DYNAMIC_FLAGS, FIELD_UNIT_LEVEL,
        };
        const ENTRY: u32 = 1234;
        let mob = (u64::from(ENTRY) << 24) | (u64::from(guid::HIGH_UNIT) << 48) | 1;
        let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
        let (tx, _rx) = crossbeam_channel::unbounded();
        app.world_mut().insert_resource(NetCommands(tx));
        app.world_mut().insert_resource(display_tables());
        let world = app.world_mut();
        super::super::handlers::dispatch(
            world,
            super::wire(vec![displayed(mob, EntityKind::Unit, 9, 10)]),
        );
        let values = |world: &mut World, pairs: &[(u16, u32)]| {
            world.resource_mut::<Messages<FieldChanged>>().clear();
            world
                .resource_mut::<Messages<crate::entities::DisplayRebuilt>>()
                .clear();
            super::super::handlers::dispatch(
                world,
                super::wire(vec![SessionEvent::ObjectValues {
                    guid: mob,
                    fields: ObjectFields::from_pairs(pairs),
                }]),
            );
            let edges: Vec<FieldChanged> = world
                .resource_mut::<Messages<FieldChanged>>()
                .drain()
                .collect();
            let rebuilds: Vec<crate::entities::DisplayRebuilt> = world
                .resource_mut::<Messages<crate::entities::DisplayRebuilt>>()
                .drain()
                .collect();
            (edges, rebuilds)
        };

        let (_, rebuilds) = values(world, &[(FIELD_UNIT_DISPLAYID, 12)]);
        assert!(rebuilds.is_empty(), "display 12 names display 10's records");

        let (edges, rebuilds) = values(
            world,
            &[
                (FIELD_UNIT_LEVEL, 2),
                (FIELD_UNIT_DISPLAYID, 11),
                (FIELD_UNIT_DYNAMIC_FLAGS, 1),
            ],
        );
        let at = |index| edges.iter().find(|e| e.index == index).unwrap().at;
        let [rebuild] = rebuilds[..] else {
            panic!("one rebuild: {rebuilds:?}");
        };
        assert_eq!(rebuild.entity, world.resource::<GuidIndex>().0[&mob]);
        assert_eq!(rebuild.display_id, 11, "the row it resolved");
        assert_eq!(
            rebuild.at,
            at(FIELD_UNIT_DISPLAYID),
            "at the display's watcher"
        );
        assert!(at(FIELD_UNIT_LEVEL) < rebuild.at && rebuild.at < at(FIELD_UNIT_DYNAMIC_FLAGS));
    }

    /// A creature asks its record again when its display names a new record set: the display
    /// watcher (`0x6049b5`) runs `0x60abe0`, whose `0x60ae10` finds the records changed, and the
    /// resolve asks (`0x60ac21` → `0x60b15b`). A display with the same records, or one with no
    /// row (`0x60ae36`), changes nothing; a player never asks (`0x60b134`).
    #[test]
    fn a_new_display_record_set_asks_the_creature_record_again() {
        use benilla_protocol::field::FIELD_UNIT_DISPLAYID;
        const ENTRY: u32 = 1234;
        let mob = (u64::from(ENTRY) << 24) | (u64::from(guid::HIGH_UNIT) << 48) | 1;
        const PLAYER: u64 = 0x0000_0000_0000_0042;
        let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
        crate::lanes::tap_tests::tap(&mut app);
        let (tx, rx) = crossbeam_channel::unbounded();
        app.world_mut().insert_resource(NetCommands(tx));
        app.world_mut().insert_resource(display_tables());
        let display = |world: &mut World, guid: u64, id: u32| {
            super::super::handlers::dispatch(
                world,
                super::wire(vec![SessionEvent::ObjectValues {
                    guid,
                    fields: ObjectFields::from_pairs(&[(FIELD_UNIT_DISPLAYID, id)]),
                }]),
            );
        };

        let world = app.world_mut();
        super::super::handlers::dispatch(
            world,
            super::wire(vec![
                displayed(mob, EntityKind::Unit, 9, 10),
                displayed(PLAYER, EntityKind::Player, 0x19, 10),
            ]),
        );
        assert_eq!(record_asks(&rx), (1, 0), "the creature's create hook alone");
        // Refused: the next lookup with a callback asks at once.
        world
            .resource_mut::<NameCache>()
            .insert_creature(ENTRY, None);

        display(world, mob, 12);
        assert_eq!(record_asks(&rx), (0, 0), "12 names 10's records");
        display(world, mob, 999);
        assert_eq!(record_asks(&rx), (0, 0), "no row: no change");
        display(world, PLAYER, 11);
        assert_eq!(record_asks(&rx), (0, 0), "a player resolves, never asks");
        display(world, mob, 11);
        assert_eq!(record_asks(&rx), (1, 0), "a new model: the records changed");
        display(world, mob, 11);
        assert_eq!(record_asks(&rx), (0, 0), "no edge, no watcher");
    }

    /// The health watcher rebuilds the display of a unit come back to life (`0x6047ee` →
    /// `0x604835`), whose records the display watcher did not see change: here the display table
    /// was not up when it did.
    #[test]
    fn a_unit_back_to_life_rebuilds_its_display() {
        use benilla_protocol::field::{FIELD_UNIT_DISPLAYID, FIELD_UNIT_HEALTH};
        const ENTRY: u32 = 1234;
        let mob = (u64::from(ENTRY) << 24) | (u64::from(guid::HIGH_UNIT) << 48) | 1;
        let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
        crate::lanes::tap_tests::tap(&mut app);
        let (tx, rx) = crossbeam_channel::unbounded();
        app.world_mut().insert_resource(NetCommands(tx));
        app.world_mut().insert_resource(display_tables());
        let world = app.world_mut();
        let values = |world: &mut World, pairs: &[(u16, u32)]| {
            super::super::handlers::dispatch(
                world,
                super::wire(vec![SessionEvent::ObjectValues {
                    guid: mob,
                    fields: ObjectFields::from_pairs(pairs),
                }]),
            );
        };
        super::super::handlers::dispatch(
            world,
            super::wire(vec![displayed(mob, EntityKind::Unit, 9, 10)]),
        );
        world
            .resource_mut::<NameCache>()
            .insert_creature(ENTRY, None);
        let _ = record_asks(&rx);

        let tables = world
            .remove_resource::<crate::entities::Creatures>()
            .unwrap();
        values(world, &[(FIELD_UNIT_DISPLAYID, 11)]);
        world.insert_resource(tables);
        values(world, &[(FIELD_UNIT_HEALTH, 0)]);
        assert_eq!(record_asks(&rx), (0, 0), "dying rebuilds nothing");
        values(world, &[(FIELD_UNIT_HEALTH, 50)]);
        assert_eq!(record_asks(&rx), (1, 0), "coming back does");
    }

    /// A create block for a live item runs the watchers over what it moved, never the create's
    /// record set-up (`0x465d79`, the object already shown).
    #[test]
    fn a_re_create_of_a_live_item_runs_its_watchers() {
        use crate::item_watch::ItemNotice;
        use benilla_protocol::field::FIELD_ITEM_STACK_COUNT;
        use benilla_protocol::messages::ObjectType;
        const POTIONS: u64 = 0x4000_0000_0000_0042;
        let mut app = watch_world(&[(POTIONS, 118, false)]);
        let world = app.world_mut();
        let SessionEvent::ItemCreate { fields, .. } = watched_item(POTIONS, 118, false) else {
            unreachable!()
        };
        let mut fields = fields;
        fields.merge(ObjectFields::from_pairs(&[(FIELD_ITEM_STACK_COUNT, 4)]));
        let again = SessionEvent::ItemCreate {
            guid: POTIONS,
            container: false,
            fields: fields.into_created(ObjectType::Item),
        };
        super::super::handlers::dispatch(world, super::wire(vec![again]));
        assert_eq!(
            drain_notices(world),
            [ItemNotice::StackCount {
                guid: POTIONS,
                entry: 118
            }]
        );
    }

    /// The out-of-range list releases our item through the same deactivate as a destroy
    /// (`0x464920`); our own player in the list is skipped.
    #[test]
    fn an_item_of_ours_out_of_range_is_destroyed_for_the_watchers() {
        use crate::item_watch::ItemNotice;
        const POTIONS: u64 = 0x4000_0000_0000_0042;
        let mut app = watch_world(&[(POTIONS, 118, false)]);
        let world = app.world_mut();
        super::super::handlers::dispatch(
            world,
            super::wire(vec![SessionEvent::ObjectsRemoved(vec![WATCH_ME, POTIONS])]),
        );
        assert_eq!(
            drain_notices(world),
            [ItemNotice::Destroyed {
                guid: POTIONS,
                entry: 118
            }]
        );
    }

    /// A bag seated in one of our bag seats has its own seats watched (`0x4f90d0`); the same bag
    /// before it is seated, or another in the backpack, has none.
    #[test]
    fn a_seated_bags_seats_run_their_watcher_through_the_handlers() {
        use crate::item_watch::ItemNotice;
        use benilla_protocol::field::{FIELD_CONTAINER_SLOT_1, FIELD_PLAYER_INV_SLOT_HEAD};
        const POTIONS: u64 = 0x4000_0000_0000_0042;
        const BAG: u64 = 0x4000_0000_0000_0050;
        let mut app = watch_world(&[(POTIONS, 118, false), (BAG, 4496, true)]);
        let world = app.world_mut();
        let seat = |j: u16, g: u64| SessionEvent::ObjectValues {
            guid: BAG,
            fields: ObjectFields::from_pairs(&[
                (FIELD_CONTAINER_SLOT_1 + 2 * j, g as u32),
                (FIELD_CONTAINER_SLOT_1 + 2 * j + 1, (g >> 32) as u32),
            ]),
        };
        let bag_seat = FIELD_PLAYER_INV_SLOT_HEAD + 2 * 19;
        super::super::handlers::dispatch(
            world,
            super::wire(vec![
                seat(1, POTIONS),
                SessionEvent::ObjectValues {
                    guid: WATCH_ME,
                    fields: ObjectFields::from_pairs(&[
                        (bag_seat, BAG as u32),
                        (bag_seat + 1, (BAG >> 32) as u32),
                    ]),
                },
                seat(1, 0),
                seat(2, POTIONS),
            ]),
        );
        assert_eq!(
            drain_notices(world),
            [
                ItemNotice::Bag { entries: vec![118] },
                ItemNotice::Seat {
                    old: Some(118),
                    new: None
                },
                ItemNotice::Seat {
                    old: None,
                    new: Some(118)
                },
            ],
            "the unseated bag's change ran nothing; the seated one's two did"
        );
    }

    /// `0x1EB` for an unheld item waits on the active player, for the item's record to replay it
    /// ([`a_queued_enchant_time_replays_when_the_items_record_applies`]); `0x1EA` for one is
    /// dropped; the destroy takes the cells.
    #[test]
    fn item_countdowns_live_on_the_item_and_a_queued_enchant_waits_for_it() {
        use crate::items::Countdowns;
        const ME: u64 = 0x0000_0000_0000_0007;
        const SWORD: u64 = 0x4000_0000_0000_0042;
        const STONE: u64 = 0x4000_0000_0000_0043;
        let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
        crate::lanes::tap_tests::tap(&mut app);
        let world = app.world_mut();
        let create = |guid| SessionEvent::ItemCreate {
            guid,
            container: false,
            fields: ObjectFields::from_pairs(&[(3, 117)]),
        };
        let cells = |world: &World, guid: u64| -> Option<Countdowns> {
            let e = *world.resource::<GuidIndex>().0.get(&guid)?;
            world.get::<Countdowns>(e).cloned()
        };

        // No active player: the enchant update has nowhere to wait.
        super::super::handlers::dispatch(
            world,
            super::wire(vec![
                SessionEvent::ItemEnchantTime {
                    item_guid: STONE,
                    slot: 1,
                    seconds: 60,
                },
                create(STONE),
            ]),
        );
        assert_eq!(
            cells(world, STONE),
            Some(Countdowns::default()),
            "dropped, not queued"
        );

        // With the active player resolving, the enchant waits for the item; the lifetime does not.
        let me = world.spawn_empty().id();
        world.resource_mut::<GuidIndex>().0.insert(ME, me);
        world.resource_mut::<SelfGuid>().0 = Some(ME);
        super::super::handlers::dispatch(
            world,
            super::wire(vec![
                SessionEvent::ItemEnchantTime {
                    item_guid: SWORD,
                    slot: 1,
                    seconds: 90,
                },
                SessionEvent::ItemTime {
                    item_guid: SWORD,
                    seconds: 600,
                },
            ]),
        );
        assert_eq!(cells(world, SWORD), None, "nothing held yet");
        super::super::handlers::dispatch(world, super::wire(vec![create(SWORD)]));
        let c = cells(world, SWORD).expect("the create spawned the item with its cells");
        assert_eq!(
            c.enchant_remaining_ms(1),
            None,
            "the enchant waits on the record, which no one has sent"
        );
        assert_eq!(
            c.lifetime_remaining_ms(),
            None,
            "`0x1EA` for an unheld item has no queue"
        );

        // Held now: both arms write the cells directly; a slot past the array writes nothing.
        super::super::handlers::dispatch(
            world,
            super::wire(vec![
                SessionEvent::ItemTime {
                    item_guid: SWORD,
                    seconds: 600,
                },
                SessionEvent::ItemEnchantTime {
                    item_guid: SWORD,
                    slot: 1,
                    seconds: 30,
                },
                SessionEvent::ItemEnchantTime {
                    item_guid: SWORD,
                    slot: 9,
                    seconds: 30,
                },
            ]),
        );
        let c = cells(world, SWORD).unwrap();
        assert!(c.lifetime_remaining_ms().is_some());
        assert!(c.enchant_remaining_ms(1).is_some());

        // The destroy takes the object and its cells with it.
        let e = world.resource::<GuidIndex>().0[&SWORD];
        super::super::handlers::dispatch(
            world,
            super::wire(vec![SessionEvent::ObjectDestroyed(SWORD)]),
        );
        assert!(!world.resource::<GuidIndex>().0.contains_key(&SWORD));
        assert!(world.get_entity(e).is_err(), "the cells died with the item");
    }

    /// A queued `0x1EB` replays from the item's record application, `0x5d8440` (`0x5ebde0` at
    /// `0x5d85b7`): at the create hook when the record is cached, else when it lands (`0x5d8390`),
    /// and only for the active player's item while in the world (`0x5d8504`, `0x5d8521`).
    #[test]
    fn a_queued_enchant_time_replays_when_the_items_record_applies() {
        use crate::items::Countdowns;
        use benilla_protocol::messages::ObjectType;
        const ME: u64 = 0x0000_0000_0000_0007;
        const SWORD: u64 = 0x4000_0000_0000_0042;
        const THEIRS: u64 = 0x4000_0000_0000_0043;
        const MACE: u64 = 0x4000_0000_0000_0044;
        const ENTRY: u32 = 117;
        let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
        crate::lanes::tap_tests::tap(&mut app);
        let world = app.world_mut();
        let me = world
            .spawn((
                Guid(ME),
                ObjectStore(ObjectFields::default().into_created(ObjectType::Player)),
            ))
            .id();
        world.resource_mut::<GuidIndex>().0.insert(ME, me);
        world.resource_mut::<SelfGuid>().0 = Some(ME);
        let create = |guid: u64, owner: u64| SessionEvent::ItemCreate {
            guid,
            container: false,
            fields: ObjectFields::from_pairs(&[
                (2, 0x3),
                (3, ENTRY),
                (6, owner as u32),
                (7, (owner >> 32) as u32),
            ])
            .into_created(ObjectType::Item),
        };
        let queue = |guid: u64| SessionEvent::ItemEnchantTime {
            item_guid: guid,
            slot: 1,
            seconds: 90,
        };
        let replayed = |world: &World, guid: u64| -> bool {
            let e = world.resource::<GuidIndex>().0[&guid];
            world
                .get::<Countdowns>(e)
                .and_then(|c| c.enchant_remaining_ms(1))
                .is_some_and(|ms| (89_000..=90_000).contains(&ms))
        };

        super::super::handlers::dispatch(
            world,
            super::wire(vec![
                queue(SWORD),
                queue(THEIRS),
                create(SWORD, ME),
                create(THEIRS, 0x99),
            ]),
        );
        assert!(!replayed(world, SWORD), "no record yet: nothing applies");
        super::super::handlers::dispatch(
            world,
            vec![SessionEvent::ItemTemplate {
                entry: ENTRY,
                info: Some(Box::new(crate::items::test_template("Sword"))),
            }],
        );
        assert!(replayed(world, SWORD), "the record landed and applied");
        assert!(!replayed(world, THEIRS), "another's item applies no replay");

        super::super::handlers::dispatch(world, super::wire(vec![queue(MACE), create(MACE, ME)]));
        assert!(
            replayed(world, MACE),
            "a cached record applies at the create hook"
        );
    }

    /// The observer movement-mode family, on a bare `World` with one indexed unit.
    mod spline_modes {
        use super::*;
        use benilla_protocol::SplineMode;
        use bevy::ecs::system::RunSystemOnce;

        const MOB: u64 = 0x0000_0000_0000_00F0;

        /// A world holding one indexed unit, optionally already dead-reckoning as a relayed player.
        fn world_with_unit(remote: Option<RemoteMotion>) -> (World, Entity) {
            let mut w = World::new();
            let mut e = w.spawn_empty();
            if let Some(rm) = remote {
                e.insert(rm);
            }
            let entity = e.id();
            let mut index = GuidIndex::default();
            index.0.insert(MOB, entity);
            w.insert_resource(index);
            (w, entity)
        }

        fn remote_walking_forward() -> RemoteMotion {
            RemoteMotion {
                wow_pos: [0.0; 3],
                pending: std::collections::VecDeque::new(),
                orientation: 0.0,
                flags: crate::creature_anim::move_flags::FORWARD,
                pitch: 0.0,
                speed: 7.0,
                vertical_velocity: 0.0,
                jump_xy_vel: [0.0; 2],
                fall_start: None,
                relay: Default::default(),
                last_apply_ms: 0.0,
                last_apply_pos: [0.0; 3],
            }
        }

        /// Runs one grant and flushes its commands.
        fn grant(w: &mut World, mode: SplineMode, apply: bool) {
            w.run_system_once(
                move |mut commands: Commands,
                      index: Res<GuidIndex>,
                      mut modes: Query<&mut UnitMoveModes>,
                      mut remote: Query<&mut RemoteMotion>| {
                    spline_move_mode(
                        MOB,
                        mode,
                        apply,
                        &mut commands,
                        &index,
                        &mut modes,
                        &mut remote,
                    );
                },
            )
            .unwrap();
        }

        /// Every one of the twelve opcodes sets or clears one bit.
        #[test]
        fn a_grant_sets_the_bit_and_its_pair_clears_it() {
            use crate::creature_anim::move_flags;
            let (mut w, e) = world_with_unit(None);

            grant(&mut w, SplineMode::Hover, true);
            grant(&mut w, SplineMode::WaterWalk, true);
            let m = *w
                .entity(e)
                .get::<UnitMoveModes>()
                .expect("first grant inserts");
            assert_eq!(
                m.0,
                move_flags::HOVER | move_flags::WATER_WALKING,
                "two grants accumulate in one word — the reference has one CMovement+0x40"
            );

            grant(&mut w, SplineMode::Hover, false);
            let m = *w.entity(e).get::<UnitMoveModes>().unwrap();
            assert_eq!(
                m.0,
                move_flags::WATER_WALKING,
                "UNSET_HOVER takes back hover and nothing else"
            );
        }

        /// `SET_RUN_MODE` clears the walk bit: the pair is inverted against its names (`0x617e80`
        /// → `SetRunMode` `0x7c71c0`), folded at the parse.
        #[test]
        fn the_walk_bit_round_trips_with_the_run_opcodes_sense() {
            use crate::creature_anim::move_flags;
            let (mut w, e) = world_with_unit(None);
            grant(&mut w, SplineMode::WalkMode, true); // SMSG_SPLINE_MOVE_SET_WALK_MODE
            assert_eq!(
                w.entity(e).get::<UnitMoveModes>().unwrap().0,
                move_flags::WALK_MODE
            );
            grant(&mut w, SplineMode::WalkMode, false); // SMSG_SPLINE_MOVE_SET_RUN_MODE
            assert_eq!(w.entity(e).get::<UnitMoveModes>().unwrap().0, 0);
        }

        /// `SetRoot` (`0x7c7340`) wipes the direction bits, so the unit fails the integration gate
        /// (`0x616e20`, `test [esi+0x40], 0x20ff`) and is not stepped.
        #[test]
        fn a_root_wipes_the_direction_bits_the_dead_reckon_runs_on() {
            use crate::creature_anim::move_flags;
            let (mut w, e) = world_with_unit(Some(remote_walking_forward()));

            grant(&mut w, SplineMode::Root, true);

            let rm = w.entity(e).get::<RemoteMotion>().unwrap();
            assert_eq!(
                rm.flags & move_flags::INTEGRATED,
                0,
                "nothing the integration gate tests may survive the root — this is what stops it"
            );
            assert_eq!(rm.speed, 0.0, "and it is not still travelling at run speed");
            assert!(w.entity(e).get::<UnitMoveModes>().unwrap().rooted());
        }

        /// `ClearRoot` (`0x7c7370`) only clears the bit.
        #[test]
        fn an_unroot_clears_the_bit_and_invents_no_movement() {
            let (mut w, e) = world_with_unit(Some(remote_walking_forward()));
            grant(&mut w, SplineMode::Root, true);
            grant(&mut w, SplineMode::Root, false);
            assert!(!w.entity(e).get::<UnitMoveModes>().unwrap().rooted());
            assert_eq!(
                w.entity(e).get::<RemoteMotion>().unwrap().flags,
                0,
                "still no direction bits — the wipe is one-shot and the unroot does not undo it"
            );
        }

        /// As in the reference's handler (`0x603c80`); the create carries the modes itself.
        #[test]
        fn a_grant_for_a_unit_we_do_not_have_is_dropped() {
            let (mut w, e) = world_with_unit(None);
            w.run_system_once(
                |mut commands: Commands,
                 index: Res<GuidIndex>,
                 mut modes: Query<&mut UnitMoveModes>,
                 mut remote: Query<&mut RemoteMotion>| {
                    spline_move_mode(
                        0xDEAD_BEEF,
                        SplineMode::Root,
                        true,
                        &mut commands,
                        &index,
                        &mut modes,
                        &mut remote,
                    );
                },
            )
            .unwrap();
            assert!(w.entity(e).get::<UnitMoveModes>().is_none());
        }

        /// `0x6187a0` returns on ROOT (`0x6187c2`), so a queued path does not move a pinned body.
        #[test]
        fn a_rooted_unit_refuses_the_path_the_server_queued() {
            let mut w = World::new();
            let e = w
                .spawn((
                    Transform::default(),
                    NetEntity {
                        kind: EntityKind::Unit,
                        display_id: None,
                        scale: 1.0,
                    },
                ))
                .id();
            let mut index = GuidIndex::default();
            index.0.insert(MOB, e);
            w.insert_resource(index);

            let path = vec![[0.0, 0.0, 0.0], [30.0, 0.0, 0.0]];
            for (rooted, expect_spline) in [(true, false), (false, true)] {
                let path = path.clone();
                w.run_system_once(
                    move |mut commands: Commands,
                          index: Res<GuidIndex>,
                          mut transforms: Query<&mut Transform>,
                          mut riders: Query<&mut crate::transport::TransportRider>| {
                        monster_move(
                            MOB,
                            None,
                            [0.0, 0.0, 0.0],
                            7,
                            path.clone(),
                            MonsterMoveFacing::None,
                            false,
                            3000,
                            false,
                            true,
                            rooted,
                            false,
                            &mut commands,
                            &index,
                            &mut transforms,
                            &mut riders,
                        );
                    },
                )
                .unwrap();
                assert_eq!(
                    w.entity(e).get::<Spline>().is_some(),
                    expect_spline,
                    "rooted={rooted}: a pinned body takes no path, an unpinned one does"
                );
            }
        }

        /// `0x6020e8`: a spline move ends the unit's precast whatever it is, a rooted unit's too
        /// (the tail `0x6020db`-`0x6020ef` runs on every path). On our own player the end's loot
        /// release reads the flags the path just wrote: an installed path is moving
        /// (`0x7c6ad4`), so the move-start close, which keeps an item's loot, and a refused one
        /// has stopped (`0x6187b1`), so the full close, whatever we last sent; each closes as the
        /// handler's commands apply, inside its packet.
        #[test]
        fn a_spline_move_ends_the_precast() {
            const CORPSE: u64 = 0xF130_0000_0000_0042;
            const LOCKBOX: u64 = 0x4000_0000_0000_0007;
            for (rooted, ours, source) in [false, true]
                .into_iter()
                .flat_map(|r| [false, true].map(|o| (r, o)))
                .flat_map(|(r, o)| [CORPSE, LOCKBOX].map(|s| (r, o, s)))
            {
                let mut w = World::new();
                let (tx, rx) = crossbeam_channel::unbounded();
                w.insert_resource(NetCommands(tx));
                w.init_resource::<crate::ui_loot::LootState>();
                w.init_resource::<crate::ui_loot::LootLatch>();
                w.resource_mut::<crate::ui_loot::LootState>().open(
                    source,
                    benilla_protocol::messages::loot_type::CORPSE,
                    5,
                    vec![],
                );
                w.resource_mut::<crate::ui_loot::LootLatch>().0 = Some(source);
                let moving = crate::creature_anim::move_flags::FORWARD;
                w.insert_resource(crate::player::Player::with_move_flags(if rooted {
                    moving
                } else {
                    0
                }));
                let e = w
                    .spawn((
                        Transform::default(),
                        crate::creature_anim::Precast {
                            spell_id: 133,
                            busy: true,
                        },
                    ))
                    .id();
                if ours {
                    w.entity_mut(e).insert(crate::net::SelfPlayer);
                }
                let mut index = GuidIndex::default();
                index.0.insert(MOB, e);
                w.insert_resource(index);
                w.run_system_once(
                    move |mut commands: Commands,
                          index: Res<GuidIndex>,
                          mut transforms: Query<&mut Transform>,
                          mut riders: Query<&mut crate::transport::TransportRider>| {
                        monster_move(
                            MOB,
                            None,
                            [0.0, 0.0, 0.0],
                            7,
                            vec![[0.0, 0.0, 0.0], [30.0, 0.0, 0.0]],
                            MonsterMoveFacing::None,
                            false,
                            3000,
                            false,
                            true,
                            rooted,
                            false,
                            &mut commands,
                            &index,
                            &mut transforms,
                            &mut riders,
                        );
                    },
                )
                .unwrap();
                assert!(
                    w.entity(e).get::<crate::creature_anim::Precast>().is_none(),
                    "rooted={rooted}: the precast ends"
                );
                let closes = ours && (rooted || source == CORPSE);
                assert_eq!(
                    w.resource::<crate::ui_loot::LootState>().source().is_none(),
                    closes,
                    "rooted={rooted}, ours={ours}, {source:#x}: the loot release"
                );
                assert_eq!(
                    rx.try_iter()
                        .filter(
                            |c| matches!(c, ClientCommand::LootRelease { guid } if *guid == source)
                        )
                        .count(),
                    usize::from(closes),
                    "rooted={rooted}, ours={ours}, {source:#x}: released once as it closes"
                );
            }
        }

        /// A plain path takes the unit off the deck, where vmangos drops the passenger
        /// (`MoveSplineInit.cpp:156-159`).
        #[test]
        fn a_transport_path_attaches_the_rider_and_a_plain_one_lets_it_go() {
            const BOAT: u64 = 0x2000_0000_0000_0007;
            let mut w = World::new();
            let e = w
                .spawn((
                    Transform::default(),
                    NetEntity {
                        kind: EntityKind::Unit,
                        display_id: None,
                        scale: 1.0,
                    },
                ))
                .id();
            let boat = w.spawn(Transform::default()).id();
            let mut index = GuidIndex::default();
            index.0.insert(MOB, e);
            index.0.insert(BOAT, boat);
            w.insert_resource(index);

            let deck_path = vec![[1.5, -2.5, 0.75], [4.0, -2.5, 0.75]];
            w.run_system_once(
                move |mut commands: Commands,
                      index: Res<GuidIndex>,
                      mut transforms: Query<&mut Transform>,
                      mut riders: Query<&mut crate::transport::TransportRider>| {
                    monster_move(
                        MOB,
                        Some(BOAT),
                        [1.5, -2.5, 0.75],
                        7,
                        deck_path.clone(),
                        MonsterMoveFacing::Angle(1.25),
                        false,
                        3000,
                        false,
                        true,
                        false,
                        false,
                        &mut commands,
                        &index,
                        &mut transforms,
                        &mut riders,
                    );
                },
            )
            .unwrap();
            let rider = w
                .entity(e)
                .get::<crate::transport::TransportRider>()
                .expect("a transport path attaches its rider");
            assert_eq!(rider.transport_guid, BOAT);
            assert_eq!(rider.local_pos, [1.5, -2.5, 0.75], "seeded deck-local");
            assert_eq!(
                rider.local_orientation, 0.0,
                "a path that installs keeps its facing for its end"
            );
            assert!(matches!(
                w.entity(e).get::<Spline>().expect("a path").final_facing,
                MonsterMoveFacing::Angle(a) if a == 1.25
            ));
            assert_eq!(
                w.entity(e).get::<Spline>().expect("a path").deck,
                Some(BOAT),
                "the spline rides the deck frame, not the world"
            );
            // The deck-local start is never written as a world position.
            assert_eq!(
                w.entity(e).get::<Transform>().unwrap().translation,
                Vec3::ZERO
            );

            let world_path = vec![[100.0, 200.0, 30.0], [110.0, 200.0, 30.0]];
            w.run_system_once(
                move |mut commands: Commands,
                      index: Res<GuidIndex>,
                      mut transforms: Query<&mut Transform>,
                      mut riders: Query<&mut crate::transport::TransportRider>| {
                    monster_move(
                        MOB,
                        None,
                        [100.0, 200.0, 30.0],
                        8,
                        world_path.clone(),
                        MonsterMoveFacing::None,
                        false,
                        3000,
                        false,
                        true,
                        false,
                        false,
                        &mut commands,
                        &index,
                        &mut transforms,
                        &mut riders,
                    );
                },
            )
            .unwrap();
            assert!(
                w.entity(e)
                    .get::<crate::transport::TransportRider>()
                    .is_none(),
                "a plain path takes the unit off the deck"
            );
            assert_eq!(w.entity(e).get::<Spline>().expect("a path").deck, None);
        }

        /// Runs one `monster_move` of `facing` at `start` on `e` (guid `MOB`); `duration_ms` 0
        /// installs no path.
        fn arrive(
            w: &mut World,
            start: [f32; 3],
            facing: MonsterMoveFacing,
            duration_ms: u32,
            smooth: bool,
        ) {
            w.run_system_once(
                move |mut commands: Commands,
                      index: Res<GuidIndex>,
                      mut transforms: Query<&mut Transform>,
                      mut riders: Query<&mut crate::transport::TransportRider>| {
                    monster_move(
                        MOB,
                        None,
                        start,
                        9,
                        vec![start, [start[0] + 10.0, start[1], start[2]]],
                        facing,
                        false,
                        duration_ms,
                        false,
                        true,
                        false,
                        smooth,
                        &mut commands,
                        &index,
                        &mut transforms,
                        &mut riders,
                    );
                },
            )
            .unwrap();
        }

        fn standing_unit(w: &mut World) -> Entity {
            let e = w
                .spawn((
                    Transform::default(),
                    NetEntity {
                        kind: EntityKind::Unit,
                        display_id: None,
                        scale: 1.0,
                    },
                    ObjectStore::default(),
                ))
                .id();
            let mut index = GuidIndex::default();
            index.0.insert(MOB, e);
            w.insert_resource(index);
            e
        }

        fn yaw(w: &World, e: Entity) -> f32 {
            let r = w.entity(e).get::<Transform>().unwrap().rotation;
            r.to_euler(EulerRot::YXZ).0
        }

        /// A path that installs stores the facing (`0x601fee`); the unit is not turned on arrival.
        #[test]
        fn a_path_that_installs_does_not_turn_the_unit_on_arrival() {
            let mut w = World::new();
            let e = standing_unit(&mut w);
            arrive(&mut w, [0.0; 3], MonsterMoveFacing::Angle(2.0), 3000, false);
            assert_eq!(yaw(&w, e), 0.0);
            assert!(w.entity(e).get::<Spline>().is_some());
        }

        /// No path (`0x60205e`): the raw facing turns now, and a smoothed unit's display follows
        /// through the smoother rather than snapping; it is never reset.
        #[test]
        fn with_no_path_the_unit_turns_through_the_smoother() {
            let mut w = World::new();
            w.init_resource::<crate::ui_session::InteractNpc>();
            let e = standing_unit(&mut w);
            arrive(&mut w, [0.0; 3], MonsterMoveFacing::Angle(1.0), 0, true);
            assert_eq!(yaw(&w, e), 0.0, "the display is not snapped");
            w.run_system_once(crate::net::motion::drive_display_facing)
                .unwrap();
            assert!((yaw(&w, e) - 0.5).abs() < 1e-5, "half way: {}", yaw(&w, e));
            for _ in 0..16 {
                w.run_system_once(crate::net::motion::drive_display_facing)
                    .unwrap();
            }
            assert!((yaw(&w, e) - 1.0).abs() < 1e-5, "settled: {}", yaw(&w, e));
        }

        /// A body the smoother does not govern shows the facing at once; a `Spot` heads from the
        /// unit's own position (`0x6020a5`), not the packet's start.
        #[test]
        fn with_no_path_an_unsmoothed_body_faces_its_spot_from_where_it_stands() {
            let mut w = World::new();
            let e = standing_unit(&mut w);
            arrive(
                &mut w,
                [5.0, 5.0, 0.0],
                MonsterMoveFacing::Spot([0.0, 10.0, 0.0]),
                0,
                false,
            );
            let got = yaw(&w, e);
            assert!((got - std::f32::consts::FRAC_PI_2).abs() < 1e-5, "{got}");
        }
    }
}
