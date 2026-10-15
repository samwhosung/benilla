//! The honor feed: our own honor descriptor fields become the snapshot the Honor tabs read, and
//! the inspect-honor reply is matched to the inspected player. The honor block streams only to
//! its owner, so another player's numbers come only from `MSG_INSPECT_HONOR_STATS`, which vmangos
//! refuses by not answering (`MiscHandler.cpp:962-972`); the stock pane re-asks from its `OnShow`
//! while it holds nothing, the only retry.
//!
//! The pane's events are three field watches (`0x467e70`, callback `0x5de4b0`):
//! `PLAYER_FIELD_SESSION_KILLS` fires `PLAYER_PVP_KILLS_CHANGED`, and the whole `PLAYER_BYTES_3`
//! dword and `PLAYER_FIELD_BYTES2` byte 0 fire `PLAYER_PVP_RANK_CHANGED`, from
//! [`crate::field_watch`]; this feed pushes what their handlers read, ahead of them and of the
//! world-enter events. Nothing watches the weekly, lifetime or highest-rank fields, which
//! `HonorFrame.lua:10-11` repaints only on `PLAYER_ENTERING_WORLD`.

use std::collections::HashMap;

use bevy::prelude::*;

use benilla_ui::script::{HonorState, InspectHonorData, ScriptValue, UiScript};

use crate::net::{ClientCommand, NetCommands, ObjectStore, SelfPlayer};
use crate::ui_script::{UiFeed, VmMemo};

/// The inspect-honor reply held: one slot, like the reference's `HasInspectHonorData` latch.
#[derive(Resource, Default)]
pub(crate) struct InspectHonor(pub(crate) Option<benilla_protocol::messages::InspectHonorStats>);

/// What the last push told this VM, keyed on the VM so a `/reload`'s fresh VM is told again.
#[derive(Resource, Default)]
pub(crate) struct HonorFeedState {
    vm: VmMemo<HonorFeedMemo>,
}

/// The per-VM change bases: the last snapshot and the held reply's guid.
#[derive(Default)]
struct HonorFeedMemo {
    last: Option<HonorState>,
    last_inspect: Option<u64>,
}

/// The inspect-honor reply's handler; the honor award, `SMSG_PVP_CREDIT`, is the chat family's.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::InspectHonor;
    use crate::net::NetHandlerApp;

    pub(super) fn register(app: &mut App) {
        app.net_handler(SessionEventKind::InspectHonorStats, on_inspect_stats);
    }

    /// A reply replaces whatever is held, whoever it is for: the reference's latch is one slot.
    fn on_inspect_stats(In(ev): In<SessionEvent>, mut inspect: ResMut<InspectHonor>) {
        if let SessionEvent::InspectHonorStats(stats) = ev {
            inspect.0 = Some(stats);
        }
    }
}

pub(crate) struct UiHonorPlugin;

impl Plugin for UiHonorPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<InspectHonor>()
            .init_resource::<HonorFeedState>()
            .init_resource::<crate::nameplates::PvpNameStrings>()
            // Ahead of the unit feed: the reference writes the block with the create whose hook
            // runs the world-enter cascade, so `PLAYER_ENTERING_WORLD`'s repaint reads it, and
            // the field events' handlers read it too.
            .add_systems(
                Update,
                feed_honor
                    .in_set(UiFeed)
                    .in_set(crate::ui_unit::field_events::FieldEventPushes)
                    .before(crate::ui_unit::UnitFeed),
            )
            // A VM holder, so after the tick; before the driver, so a new VM's strings reach the
            // lines the frame it arrives.
            .add_systems(
                Update,
                feed_pvp_name_strings
                    .after(crate::ui_script::UiInput)
                    .before(crate::nameplates::drive_nameplates),
            );
    }
}

/// The overhead name's rank strings ([`crate::nameplates::PvpNameStrings`]), read off a VM the
/// first frame it is in the world. Every VM is installed whole, outside `Update` (the boot VM with
/// `GlobalStrings.lua` run, a world VM after its FrameXML and addon load), so the read sees what
/// the load defined; any other frame pays one compare.
fn feed_pvp_name_strings(
    script: Option<NonSend<UiScript>>,
    mut strings: ResMut<crate::nameplates::PvpNameStrings>,
) {
    let Some(script) = script else {
        return;
    };
    if strings.session() != Some(script.session()) {
        strings.set(script.session(), pvp_name_strings(&script));
    }
}

/// The [`benilla_ui::script::pvp_name_global_keys`] that `script` defines, with their values.
fn pvp_name_strings(script: &UiScript) -> HashMap<String, String> {
    benilla_ui::script::pvp_name_global_keys()
        .into_iter()
        .filter_map(|key| benilla_ui::strings::global(script.lua(), &key).map(|value| (key, value)))
        .collect()
}

/// Read the honor block off our own descriptor, `None` until it streams. That decides only when
/// the first push and its events happen: the reference reads a field it was never sent as 0
/// (`0x51a4b0`-`0x51a7c0`), and so do our bindings.
fn honor_snapshot(store: &ObjectStore) -> Option<HonorState> {
    let f = &store.0;
    let session = f.player_session_kills();
    let yesterday = f.player_yesterday_kills();
    let last_week = f.player_last_week_kills();
    let this_week = f.player_this_week_kills();
    // Presence is tested on a private field: the public rank byte streams with the unit block.
    session?;
    Some(HonorState {
        session_hk: session.map_or(0, |(hk, _)| hk),
        session_dk: session.map_or(0, |(_, dk)| dk),
        yesterday_hk: yesterday.map_or(0, |(hk, _)| hk),
        yesterday_dk: yesterday.map_or(0, |(_, dk)| dk),
        yesterday_honor: f.player_yesterday_contribution().unwrap_or(0),
        this_week_hk: this_week.map_or(0, |(hk, _)| hk),
        this_week_honor: f.player_this_week_contribution().unwrap_or(0),
        last_week_hk: last_week.map_or(0, |(hk, _)| hk),
        last_week_dk: last_week.map_or(0, |(_, dk)| dk),
        last_week_honor: f.player_last_week_contribution().unwrap_or(0),
        last_week_standing: f.player_last_week_rank().unwrap_or(0),
        lifetime_hk: f.player_lifetime_honorable_kills().unwrap_or(0),
        lifetime_dk: f.player_lifetime_dishonorable_kills().unwrap_or(0),
        // The highest lifetime rank, private `PLAYER_FIELD_BYTES` byte 3.
        highest_rank: f.player_honor_rank().unwrap_or(0),
        // The current rank, public `PLAYER_BYTES_3` byte 3: another field, equal to the highest
        // until the player ranks down, so a swap would pass unnoticed.
        rank: f.player_pvp_rank().unwrap_or(0),
        rank_bar: f.player_honor_rank_bar().unwrap_or(0),
    })
}

/// Push the self snapshot and the inspect reply, and drain the pane's request.
pub(crate) fn feed_honor(
    script: Option<NonSendMut<UiScript>>,
    self_store: Query<&ObjectStore, With<SelfPlayer>>,
    inspect_target: Res<crate::ui_inspect::InspectTarget>,
    selection: Res<crate::target::Selection>,
    group: Res<crate::ui_party::GroupState>,
    mut inspect_honor: ResMut<InspectHonor>,
    mut state: ResMut<HonorFeedState>,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    let memo = state.vm.get(&script);

    // --- the self snapshot -------------------------------------------------------------------
    if let Ok(store) = self_store.single() {
        if let Some(fresh) = honor_snapshot(store) {
            if memo.last.as_ref() != Some(&fresh) {
                script.set_honor(Some(fresh));
                memo.last = Some(fresh);
            }
        }
    }

    // --- the inspect reply -------------------------------------------------------------------
    // The reference's latch clears only at `0x4c6f70`, from a `NotifyInspect` naming another
    // player, and on `ClearInspectPlayer`. Deviation: we clear it whenever the inspected token's
    // guid moves, because our window re-resolves its token each frame and both tabs read it, so
    // the reference's rule would let the two tabs show different players. `ClearInspectPlayer`
    // drops the token, which clears the slot here too.
    let inspected = inspect_target
        .token
        .as_deref()
        .and_then(|t| crate::ui_unit::player_token_guid(t, &selection, &group));
    if inspect_honor
        .0
        .as_ref()
        .is_some_and(|reply| Some(reply.player_guid) != inspected)
    {
        // Dropped from the resource, so `HasInspectHonorData` and this store agree.
        inspect_honor.0 = None;
    }
    let held = inspect_honor.0.as_ref().map(|r| r.player_guid);
    if memo.last_inspect != held {
        script.set_inspect_honor(inspect_honor.0.as_ref().map(|r| {
            let (session_hk, session_dk) = r.session_kills();
            InspectHonorData {
                guid: r.player_guid,
                session_hk,
                session_dk,
                yesterday_hk: r.yesterday_hk,
                yesterday_honor: r.yesterday_honor,
                this_week_hk: r.this_week_hk,
                this_week_honor: r.this_week_honor,
                last_week_hk: r.last_week_hk,
                last_week_honor: r.last_week_honor,
                last_week_standing: r.last_week_rank,
                lifetime_hk: r.lifetime_hk,
                lifetime_dk: r.lifetime_dhk,
                highest_rank: r.highest_rank,
                rank_bar: r.rank_bar,
            }
        }));
        memo.last_inspect = held;
        // Fired on a clear too, so the pane stops showing the previous player's numbers.
        script.fire_event("INSPECT_HONOR_UPDATE", Vec::<ScriptValue>::new());
    }

    // --- the pane's request ------------------------------------------------------------------
    // `RequestInspectHonorData()` takes no argument: it asks about the inspected token's guid now,
    // so a re-target before the Honor tab opens asks about the new player.
    let requests = script.take_inspect_honor_requests();
    if requests > 0 {
        match inspected {
            Some(guid) => {
                debug!("honor: inspect-honor request -> {guid:#x}");
                let _ = commands
                    .0
                    .send(ClientCommand::InspectHonorStats { target: guid });
            }
            // Nothing to ask about; the pane asks again from its next `OnShow`.
            None => debug!("honor: inspect-honor request with no resolvable target — not sent"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The block is pushed on first sight and on change, and the push fires nothing: the PvP
    /// events are the field watches' (`0x5ddc35`, `0x5ddc5c`, `0x5ddc83`), which a create and a
    /// `/reload` never run.
    #[test]
    fn the_feed_pushes_the_block_and_fires_nothing() {
        use bevy::ecs::system::RunSystemOnce;
        const FIELD_SESSION_KILLS: u16 = 1250;
        let mut app = App::new();
        app.init_resource::<HonorFeedState>()
            .init_resource::<InspectHonor>()
            .init_resource::<crate::ui_inspect::InspectTarget>()
            .init_resource::<crate::target::Selection>()
            .init_resource::<crate::ui_party::GroupState>();
        let (tx, _rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        let script = vm(r#"SEEN = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("PLAYER_PVP_RANK_CHANGED")
            f:RegisterEvent("PLAYER_PVP_KILLS_CHANGED")
            f:SetScript("OnEvent", function()
                table.insert(SEEN, event .. " " .. tostring(arg1))
            end)"#);
        app.insert_non_send_resource(script);
        let me = app
            .world_mut()
            .spawn((
                SelfPlayer,
                ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(
                    FIELD_SESSION_KILLS,
                    3,
                )])),
            ))
            .id();
        // The session's honorable kills as the pane reads them, and what the VM heard.
        let step = |app: &mut App| {
            app.world_mut().run_system_once(feed_honor).unwrap();
            let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
            s.resolve();
            let kills = s.eval::<i64>("return (GetPVPSessionStats())").unwrap();
            let heard = s.eval::<Vec<String>>("return SEEN").unwrap();
            s.run("SEEN = {}").unwrap();
            (kills, heard)
        };
        assert_eq!(step(&mut app), (3, vec![]), "the first push is silent");
        *app.world_mut().get_mut::<ObjectStore>(me).unwrap() = ObjectStore(
            benilla_protocol::ObjectFields::from_pairs(&[(FIELD_SESSION_KILLS, 4)]),
        );
        assert_eq!(
            step(&mut app),
            (4, vec![]),
            "a change is the watch's to fire"
        );
    }

    fn vm(src: &str) -> UiScript {
        let script = UiScript::new().expect("a VM");
        script.run(src).expect("globals set");
        script
    }

    /// The read takes the overhead name's keys a VM defines, `_FEMALE` twins included, and
    /// nothing else.
    #[test]
    fn the_rank_strings_read_takes_only_the_title_keys() {
        let strings = pvp_name_strings(&vm(r#"UNIT_PVP_NAME = "%s %s"
               PVP_RANK_7_1 = "Sergeant"
               PVP_RANK_7_1_FEMALE = "Sergeant (f)"
               GARBAGE_KEY = "x""#));
        assert_eq!(
            strings.get("UNIT_PVP_NAME").map(String::as_str),
            Some("%s %s")
        );
        assert_eq!(
            strings.get("PVP_RANK_7_1_FEMALE").map(String::as_str),
            Some("Sergeant (f)")
        );
        assert!(!strings.contains_key("GARBAGE_KEY"), "only the title keys");
    }

    /// Each VM is read once, the first frame it is in the world, so a world VM whose addon renames
    /// a title reaches the overhead line.
    #[test]
    fn each_new_vm_is_read_once() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.init_resource::<crate::nameplates::PvpNameStrings>();
        world.run_system_once(feed_pvp_name_strings).unwrap();
        assert_eq!(
            world
                .resource::<crate::nameplates::PvpNameStrings>()
                .session(),
            None,
            "no VM, no read"
        );
        let stock = r#"UNIT_PVP_NAME = "%s %s"
                       PVP_RANK_7_1 = "Sergeant""#;
        let boot = vm(stock);
        let boot_session = boot.session();
        world.insert_non_send_resource(boot);
        world.run_system_once(feed_pvp_name_strings).unwrap();
        let held = world.resource::<crate::nameplates::PvpNameStrings>();
        assert_eq!(held.session(), Some(boot_session));
        assert_eq!(held.get("PVP_RANK_7_1"), Some("Sergeant"));

        // The same VM is not read again: a global it sets later stays out.
        world
            .non_send_resource::<UiScript>()
            .run(r#"PVP_RANK_7_1 = "Later""#)
            .expect("global set");
        world.run_system_once(feed_pvp_name_strings).unwrap();
        let held = world.resource::<crate::nameplates::PvpNameStrings>();
        assert_eq!(held.get("PVP_RANK_7_1"), Some("Sergeant"));

        let world_vm = vm(&format!("{stock}\nPVP_RANK_7_1 = \"Feldwebel\""));
        let world_session = world_vm.session();
        world.insert_non_send_resource(world_vm);
        world.run_system_once(feed_pvp_name_strings).unwrap();
        let held = world.resource::<crate::nameplates::PvpNameStrings>();
        assert_eq!(held.session(), Some(world_session));
        assert_eq!(held.get("PVP_RANK_7_1"), Some("Feldwebel"));
    }
}
