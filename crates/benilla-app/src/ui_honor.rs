//! The honor state the Honor tabs read: our own honor descriptor fields, which the self getters
//! read off the active player's block at the call ([`crate::ui_inspect::InspectLive`]), and the
//! client's one inspect-honor slot ([`InspectHonor`]), which `MSG_INSPECT_HONOR_STATS` fills for
//! the inspect target. The honor block streams only to its owner, so another player's numbers
//! come only from that reply, which vmangos refuses by not answering (`MiscHandler.cpp:962-972`);
//! the stock pane re-asks from its `OnShow` while it holds nothing, the only retry.
//!
//! The pane's events are three field watches (`0x467e70`, callback `0x5de4b0`):
//! `PLAYER_FIELD_SESSION_KILLS` fires `PLAYER_PVP_KILLS_CHANGED`, and the whole `PLAYER_BYTES_3`
//! dword and `PLAYER_FIELD_BYTES2` byte 0 fire `PLAYER_PVP_RANK_CHANGED`, from
//! [`crate::field_watch`]. Nothing watches the weekly, lifetime or highest-rank fields, which
//! `HonorFrame.lua:10-11` repaints only on `PLAYER_ENTERING_WORLD`.

use std::collections::HashMap;

use bevy::prelude::*;

use benilla_ui::script::{HonorState, InspectHonorData, InspectHonorSlot, UiScript};

use crate::net::ObjectStore;

/// The client's inspect-honor slot (`ui/honorinspect`): the inspect target's guid (`0xb71fc0`),
/// the has-data (`0xb71fc8`) and pending (`0xb71fcc`) flags, and the figures of the last reply
/// for it.
#[derive(Resource, Default)]
pub(crate) struct InspectHonor {
    target: u64,
    has_data: bool,
    pending: bool,
    figures: InspectHonorData,
}

impl InspectHonor {
    /// `SetInspectTarget` (`0x4c6f70`): nothing for the guid held (`0x4c6f89`); another stores it
    /// and clears both flags (`0x4c6f98`, `0x4c6f9d`). `ClearInspectPlayer` passes 0
    /// (`0x4c9784`). Whether a held reply was dropped, which signals `INSPECT_HONOR_UPDATE`.
    ///
    /// Deviation: a new target zeroes the figures too, and dropping a held reply signals
    /// `INSPECT_HONOR_UPDATE`, where `0x4c6f70` keeps the figures and clears only has-data
    /// (`0xb71fc8`) and pending (`0xb71fcc`): a server that never answers the ask, as vmangos
    /// often does not, would leave the last player's figures shown under the new one.
    pub(crate) fn set_target(&mut self, guid: u64) -> bool {
        if guid == self.target {
            return false;
        }
        let dropped = self.has_data;
        *self = Self {
            target: guid,
            ..Self::default()
        };
        dropped
    }

    /// The inspect target's guid, 0 for none.
    pub(crate) fn target(&self) -> u64 {
        self.target
    }

    /// The request sent: pending is set after the send (`0x4c811b`).
    pub(crate) fn asked(&mut self) {
        self.pending = true;
    }

    /// The slot as the getters read it.
    pub(crate) fn slot(&self) -> InspectHonorSlot {
        InspectHonorSlot {
            has_data: self.has_data,
            pending: self.pending,
            figures: self.figures,
        }
    }

    /// The reply (`0x4c6e40`): one for another guid than the target's is dropped whole
    /// (`0x4c6e58`-`0x4c6e6b`); a match stores every value, sets has-data, clears pending
    /// (`0x4c6f42`, `0x4c6f4c`); whether it was stored.
    fn store(&mut self, r: &benilla_protocol::messages::InspectHonorStats) -> bool {
        if r.player_guid != self.target {
            return false;
        }
        let (session_hk, session_dk) = r.session_kills();
        self.figures = InspectHonorData {
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
        };
        self.has_data = true;
        self.pending = false;
        true
    }
}

/// The inspect-honor reply's handler; the honor award, `SMSG_PVP_CREDIT`, is the chat family's.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::InspectHonor;
    use crate::field_watch::{id, FieldNotice, NoticeLane};
    use crate::net::NetHandlerApp;

    pub(super) fn register(app: &mut App) {
        app.net_handler(SessionEventKind::InspectHonorStats, on_inspect_stats);
    }

    /// `0x4c6e40`: a reply for the target stored, then `INSPECT_HONOR_UPDATE` signalled at its
    /// packet (`0x4c6f3d`, `0x4c6f56`), the event's only fire site.
    fn on_inspect_stats(
        In(ev): In<SessionEvent>,
        mut slot: ResMut<InspectHonor>,
        mut lane: ResMut<NoticeLane>,
    ) {
        if let SessionEvent::InspectHonorStats(stats) = ev {
            if slot.store(&stats) {
                lane.write(FieldNotice::Global {
                    id: id::INSPECT_HONOR_UPDATE,
                });
            } else {
                debug!(
                    "honor: inspect-honor reply for {:#x}, not the target — dropped",
                    stats.player_guid
                );
            }
        }
    }
}

pub(crate) struct UiHonorPlugin;

impl Plugin for UiHonorPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<InspectHonor>()
            .init_resource::<crate::nameplates::NameLineStrings>()
            // A VM holder, so after the tick; before the driver, so a new VM's strings reach the
            // lines the frame it arrives.
            .add_systems(
                Update,
                feed_name_line_strings
                    .after(crate::ui_script::UiTick)
                    .before(crate::nameplates::drive_nameplates),
            );
    }
}

/// The overhead name's strings ([`crate::nameplates::NameLineStrings`]), read with the host lent
/// the first frame a VM is in the world and after a player line rebuilds: the reference's builder
/// reads each through `lua_gettable` (`0x70436e`, so `_G`'s `__index` runs) only when a name
/// rebuilds (`0x5ec9e0`, `0x609370`), so a steady frame runs no Lua.
fn feed_name_line_strings(
    mut vm: crate::script_calls::HostedVm,
    mut strings: ResMut<crate::nameplates::NameLineStrings>,
) {
    vm.run(|script| {
        let Some(script) = script else {
            return;
        };
        // Both read, so a new VM's read also spends a pending ask.
        if (strings.session() != Some(script.session())) | strings.take_reread() {
            strings.set(script.session(), name_line_strings(script));
        }
    });
}

/// The name line's global keys: the flag tags ([`crate::nameplates::FLAG_PREFIXES`]) and the
/// [`benilla_ui::script::pvp_name_global_keys`].
fn name_line_keys() -> impl Iterator<Item = String> {
    crate::nameplates::FLAG_PREFIXES
        .iter()
        .map(|(_, key)| key.to_string())
        .chain(benilla_ui::script::pvp_name_global_keys())
}

/// The name line's globals that `script` defines, with their values: the flag tags
/// ([`crate::nameplates::FLAG_PREFIXES`]) and the [`benilla_ui::script::pvp_name_global_keys`].
fn name_line_strings(script: &UiScript) -> HashMap<String, String> {
    name_line_keys()
        .filter_map(|key| benilla_ui::strings::global(script.lua(), &key).map(|value| (key, value)))
        .collect()
}

/// The honor block off our own descriptor as the self getters read it (`0x51a4b0`-`0x51a7c0`,
/// `0x51aa90`): a field never sent reads as 0.
pub(crate) fn honor_of(store: &ObjectStore) -> HonorState {
    let f = &store.0;
    let session = f.player_session_kills();
    let yesterday = f.player_yesterday_kills();
    let last_week = f.player_last_week_kills();
    let this_week = f.player_this_week_kills();
    HonorState {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reply for `guid` with `session` honorable kills.
    fn reply(guid: u64, session: u32) -> benilla_protocol::messages::InspectHonorStats {
        benilla_protocol::messages::InspectHonorStats {
            player_guid: guid,
            highest_rank: 0,
            session_kills: session,
            yesterday_hk: 0,
            unknown_old1: 0,
            last_week_hk: 0,
            unknown_old2: 0,
            this_week_hk: 0,
            unknown_old3: 0,
            lifetime_hk: 0,
            lifetime_dhk: 0,
            yesterday_honor: 0,
            last_week_honor: 0,
            this_week_honor: 0,
            last_week_rank: 0,
            rank_bar: 0,
        }
    }

    /// The slot holds one target (`0x4c6e58`): a reply for anyone else is dropped, one for the
    /// target sets has-data and clears pending (`0x4c6f42`, `0x4c6f4c`), the same target changes
    /// nothing (`0x4c6f89`), and a new one clears the slot whole, reporting the held reply it
    /// dropped (the slot's `Deviation:`).
    #[test]
    fn the_slot_keys_on_its_target_and_a_new_one_clears_it() {
        let mut slot = InspectHonor::default();
        assert!(!slot.store(&reply(0x77, 3)), "no target");
        assert!(!slot.set_target(0x77), "nothing held");
        slot.asked();
        assert!(!slot.store(&reply(0x88, 4)), "another player");
        assert_eq!(
            slot.slot(),
            InspectHonorSlot {
                pending: true,
                ..Default::default()
            }
        );
        assert!(slot.store(&reply(0x77, 5)));
        let held = slot.slot();
        assert!(held.has_data && !held.pending);
        assert_eq!(held.figures.session_hk, 5);
        assert!(!slot.set_target(0x77));
        assert_eq!(slot.slot(), held, "the same target");
        assert!(slot.set_target(0x99), "a held reply dropped");
        assert_eq!(slot.slot(), InspectHonorSlot::default(), "cleared whole");
        assert!(!slot.set_target(0), "nothing held");
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
        let strings = name_line_strings(&vm(r#"UNIT_PVP_NAME = "%s %s"
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

    /// The name line's flag tags are read with the titles (`0x5eca17`, `0x5eca54`, `0x5eca99`).
    #[test]
    fn the_strings_read_takes_the_chat_flag_tags() {
        let strings = name_line_strings(&vm(r#"CHAT_FLAG_AFK = "<Away>"
               CHAT_FLAG_DND = "<Busy>"
               CHAT_FLAG_GM = "<GM>""#));
        for (key, value) in [
            ("CHAT_FLAG_AFK", "<Away>"),
            ("CHAT_FLAG_DND", "<Busy>"),
            ("CHAT_FLAG_GM", "<GM>"),
        ] {
            assert_eq!(strings.get(key).map(String::as_str), Some(value), "{key}");
        }
    }

    /// Each VM is read the first frame it is in the world, so a world VM whose addon renames a
    /// title reaches the overhead line, and read again after a player line rebuilds: the builder
    /// reads the title (`0x609370`) and the flag tags (`0x5eca17`, `0x5eca54`, `0x5eca99`) at
    /// every rebuild, and only then.
    #[test]
    fn each_vm_is_read_and_read_again_after_a_rebuild() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.init_resource::<crate::nameplates::NameLineStrings>();
        world.run_system_once(feed_name_line_strings).unwrap();
        assert_eq!(
            world
                .resource::<crate::nameplates::NameLineStrings>()
                .session(),
            None,
            "no VM, no read"
        );
        let stock = r#"UNIT_PVP_NAME = "%s %s"
                       PVP_RANK_7_1 = "Sergeant""#;
        let boot = vm(stock);
        let boot_session = boot.session();
        world.insert_non_send_resource(boot);
        world.run_system_once(feed_name_line_strings).unwrap();
        let held = world.resource::<crate::nameplates::NameLineStrings>();
        assert_eq!(held.session(), Some(boot_session));
        assert_eq!(held.get("PVP_RANK_7_1"), Some("Sergeant"));

        // A global the same VM sets later waits for a rebuild, then reaches the held strings, a
        // flag tag's included.
        world
            .non_send_resource::<UiScript>()
            .run(r#"PVP_RANK_7_1 = "Later" CHAT_FLAG_AFK = "<Away>""#)
            .expect("global set");
        world.run_system_once(feed_name_line_strings).unwrap();
        let held = world.resource::<crate::nameplates::NameLineStrings>();
        assert_eq!(held.get("PVP_RANK_7_1"), Some("Sergeant"), "no rebuild yet");
        held.ask_reread();
        world.run_system_once(feed_name_line_strings).unwrap();
        let held = world.resource::<crate::nameplates::NameLineStrings>();
        assert_eq!(held.get("PVP_RANK_7_1"), Some("Later"));
        assert_eq!(held.get("CHAT_FLAG_AFK"), Some("<Away>"));

        let world_vm = vm(&format!("{stock}\nPVP_RANK_7_1 = \"Feldwebel\""));
        let world_session = world_vm.session();
        world.insert_non_send_resource(world_vm);
        world.run_system_once(feed_name_line_strings).unwrap();
        let held = world.resource::<crate::nameplates::NameLineStrings>();
        assert_eq!(held.session(), Some(world_session));
        assert_eq!(held.get("PVP_RANK_7_1"), Some("Feldwebel"));
    }

    /// The builder reads its globals through `_G`'s `__index` (`lua_gettable`, `0x70436e`) only
    /// when a name rebuilds (`0x5ec9e0`, `0x609370`), so a steady frame runs no Lua: an addon's
    /// `__index` is called by the VM's first read and not again until a rebuild asks.
    #[test]
    fn a_steady_frame_runs_no_index_metamethod() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.init_resource::<crate::nameplates::NameLineStrings>();
        world.insert_non_send_resource(vm(r#"INDEXED = 0
            setmetatable(getfenv(0), { __index = function(t, k)
                rawset(t, "INDEXED", rawget(t, "INDEXED") + 1)
            end })"#));
        let indexed = |world: &World| -> i64 {
            world
                .non_send_resource::<UiScript>()
                .eval("return rawget(getfenv(0), 'INDEXED')")
                .unwrap()
        };
        world.run_system_once(feed_name_line_strings).unwrap();
        let first = indexed(&world);
        assert!(first > 0, "the VM's first read asks the metamethod");
        for _ in 0..3 {
            world.run_system_once(feed_name_line_strings).unwrap();
        }
        assert_eq!(indexed(&world), first, "no rebuild, no read");
        world
            .resource::<crate::nameplates::NameLineStrings>()
            .ask_reread();
        world.run_system_once(feed_name_line_strings).unwrap();
        assert_eq!(indexed(&world), 2 * first, "a rebuild's read asks it again");
    }
}
