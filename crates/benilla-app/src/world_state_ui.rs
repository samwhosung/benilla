//! The always-up world-state readout behind stock `WorldStateFrame.xml`: joins the rows
//! `WorldStateUI.dbc` displays with the states the server sent ([`crate::world_state`]).
//!
//! The list builder (`0x4c56e0`) walks the whole DBC and admits a row when:
//! 1. `MapID` is -1 or the scope's map;
//! 2. `AreaID` is 0 or the scope's area;
//! 3. `Type` is 0, or 1 while the zone-defense flag is set
//!    ([`crate::ui_chat::ChannelState::defense_channel`]); 2 (battleground scoreboard columns)
//!    never.
//!
//! The scope is the server's last `SMSG_INIT_WORLD_STATES`, not the player's position: the
//! reference's two scope globals are written only by that init and by the logout reset.
//!
//! The world-PvP rows (Eastern Plaguelands, Silithus) are all `Type` 1, so leaving the zone
//! defense channel hides them; the map icons, gated on the states themselves, stay.

use bevy::prelude::*;

use benilla_formats::{WorldStateUiCatalog, WorldStateUiRow};
use benilla_ui::script::WorldStateUiView;

use benilla_assets::{AssetSet, LockRecover, WorldAssets};

use crate::world_state::{WorldStateLane, WorldStateSignal, WorldStates};

/// The shared `WorldStateUI.dbc` catalog; absent, the readout is empty.
#[derive(Resource)]
pub(crate) struct WorldStateUiRes(pub(crate) WorldStateUiCatalog);

fn load_world_state_ui(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_world_state_ui_catalog(&mut chain)
    };
    match loaded {
        Ok(cat) => {
            info!("world-state UI: {} rows in WorldStateUI.dbc", cat.len());
            commands.insert_resource(WorldStateUiRes(cat));
        }
        Err(e) => warn!("world-state UI: WorldStateUI.dbc failed — no readout: {e:#}"),
    }
}

/// Expands a `WorldStateUI` label (`0x508560`), a different expander from [`crate::npc_text`]'s,
/// sharing only the world-state getter. The grammar is `%<digits>W|w` alone, printed `%d`; any
/// other `%` emits a literal `%`, consumes its digits and leaves the next character as text.
/// Output is truncated at the reference's 256-byte limit.
fn expand(text: &str, states: &WorldStates) -> String {
    /// The `strncat` bound: `0x508560`'s caller passes `0x100`.
    const LIMIT: usize = 0x100 - 1;

    fn push(s: &str, out: &mut String) {
        for ch in s.chars() {
            if out.len() + ch.len_utf8() > LIMIT {
                return;
            }
            out.push(ch);
        }
    }

    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'%' {
            let ch = text[i..].chars().next().expect("char boundary");
            push(ch.encode_utf8(&mut [0u8; 4]), &mut out);
            i += ch.len_utf8();
            continue;
        }
        let digits_at = i + 1;
        let mut j = digits_at;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        match bytes.get(j) {
            Some(b'w' | b'W') => {
                // `atoi`: an empty run is key 0.
                let id: u32 = text[digits_at..j].parse().unwrap_or(0);
                push(&states.get(id).to_string(), &mut out);
                i = j + 1;
            }
            _ => {
                push("%", &mut out);
                i = j;
            }
        }
    }
    out
}

/// The admitted rows for the current scope, resolved as `GetWorldStateUIInfo` answers them.
fn build(
    catalog: &WorldStateUiCatalog,
    states: &WorldStates,
    defense_channel: bool,
) -> Vec<WorldStateUiView> {
    let Some((map, area)) = states.scope() else {
        // Before an init the reference never rebuilds, and its scope globals read -1.
        return Vec::new();
    };
    catalog
        .rows()
        .filter(|(_, row)| admits(row, map, area, defense_channel))
        .map(|(_, row)| resolve(row, states))
        .collect()
}

fn admits(row: &WorldStateUiRow, map: u32, area: u32, defense_channel: bool) -> bool {
    let map_ok = row.map_id == u32::MAX || row.map_id == map;
    let area_ok = row.area_id == 0 || row.area_id == area;
    let type_ok = match row.ui_type {
        0 => true,
        1 => defense_channel,
        _ => false,
    };
    map_ok && area_ok && type_ok
}

/// `GetWorldStateUIInfo` for one row (`0x4c5a70`): only the text is expanded, and the extended
/// ids answer their states' values.
fn resolve(row: &WorldStateUiRow, states: &WorldStates) -> WorldStateUiView {
    WorldStateUiView {
        // No `StateVariable` answers 1 (`0x4c5ad8`).
        ui_state: match row.state_variable {
            0 => 1,
            id => states.get(id),
        },
        text: expand(&row.text, states),
        icon: row.icon.clone(),
        dynamic_icon: row.dynamic_icon.clone(),
        tooltip: row.tooltip.clone(),
        dynamic_tooltip: row.dynamic_tooltip.clone(),
        extended_ui: row.extended_ui.clone(),
        extended_ui_state: row.extended_ui_state.map(|id| states.get(id)),
    }
}

/// Pushes the readout's rows when a world-state packet arrives or the defense-channel flag flips,
/// the reference's two rebuild triggers (`0x49bdc7`-`0x49bdcf`), once a VM is new or the frame is
/// over; the lane's firing builds them again ahead of its own signals. The flip
/// signals `UPDATE_WORLD_STATES` from here once an init has set the scope (`0x49bdc0`-`0x49bdd6`),
/// whether or not a row changed; a packet's signals are the lane's ([`fire_world_states`]). A first
/// run signals when a packet has come already, for the frames that loaded after it.
fn feed_world_state_ui(
    mut vm: crate::script_calls::VmWith<Res<'static, crate::ui_chat::ChannelState>>,
    catalog: Option<Res<WorldStateUiRes>>,
    states: Res<WorldStates>,
    mut lane: ResMut<WorldStateLane>,
    mut last: Local<crate::ui_script::VmMemo<Option<(u64, bool)>>>,
) {
    let defense_channel = vm.own().defense_channel;
    vm.run(|script| {
        let (Some(script), Some(catalog)) = (script, catalog) else {
            return;
        };
        let key = (states.generation(), defense_channel);
        let before = *last.get(script);
        if before == Some(key) {
            return;
        }
        *last.get(script) = Some(key);
        script.set_world_state_ui(build(&catalog.0, &states, defense_channel));
        let first = before.is_none() && key.0 != 0;
        let flip =
            before.is_some_and(|(_, flag)| flag != defense_channel) && states.scope().is_some();
        if first || flip {
            lane.push(WorldStateSignal::Readout);
        }
    })
}

/// The lane's firing: a packet rebuilds the world map's landmarks and the readout's rows, as its
/// handler does (`0x48f9f6` into `0x4a67a0`, `0x4c56e0`), then signals `WORLD_MAP_UPDATE` and
/// `UPDATE_WORLD_STATES`; a flip rebuilds the rows and signals the second alone. It stops after an
/// item whose firing stamped anything.
fn fire_world_states(
    In(until): In<crate::lanes::Until>,
    mut vm: crate::script_calls::VmWith<Res<'static, crate::ui_chat::ChannelState>>,
    mut lane: ResMut<WorldStateLane>,
    catalog: Option<Res<WorldStateUiRes>>,
    states: Res<WorldStates>,
    landmarks: crate::ui_world_map::LandmarkReads,
) {
    if !vm.run(|script| script.is_some()) {
        return;
    }
    while let Some(at) = lane.head().filter(|&s| until.admits(s)) {
        let Some((_, signal)) = lane.pop() else {
            return;
        };
        let defense_channel = vm.own().defense_channel;
        let ((), moved) = crate::lanes::firing(at, || {
            vm.run(|script| {
                let Some(script) = script else {
                    return;
                };
                if let Some(catalog) = catalog.as_deref() {
                    script.set_world_state_ui(build(&catalog.0, &states, defense_channel));
                }
                if signal == WorldStateSignal::Packet {
                    landmarks.rebuild_for_packet(script);
                    script.fire_event("WORLD_MAP_UPDATE", Vec::new());
                }
                script.fire_event("UPDATE_WORLD_STATES", Vec::new());
            })
        });
        if moved {
            return;
        }
    }
}

/// A signal is for the interface that was up: with none, the lane empties.
fn drop_world_state_signals(mut lane: ResMut<WorldStateLane>) {
    lane.clear();
}

pub(crate) struct WorldStateUiPlugin;

impl Plugin for WorldStateUiPlugin {
    fn build(&self, app: &mut App) {
        use crate::lanes::LaneApp;
        app.init_resource::<WorldStateLane>()
            .add_lane(
                "world states",
                |world| world.get_resource::<WorldStateLane>()?.head(),
                fire_world_states,
                None,
            )
            .add_systems(Startup, load_world_state_ui.after(AssetSet::Open))
            .add_systems(
                Update,
                (
                    feed_world_state_ui.after(crate::ui_script::UiTick),
                    drop_world_state_signals
                        .in_set(crate::ui_script::UiFeed)
                        .run_if(not(crate::ui_script::ingame_ui_up)),
                ),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_formats::chat_channel_flags as chan;

    fn row(map_id: u32, area_id: u32, ui_type: u32) -> WorldStateUiRow {
        WorldStateUiRow {
            map_id,
            area_id,
            icon: String::new(),
            text: String::new(),
            tooltip: String::new(),
            state_variable: 0,
            ui_type,
            dynamic_icon: String::new(),
            dynamic_tooltip: String::new(),
            extended_ui: String::new(),
            extended_ui_state: [0; 3],
        }
    }

    #[test]
    fn the_macro_expands_world_state_values() {
        let mut states = WorldStates::default();
        states.write(&[(2327, 3), (2328, 1)]);
        assert_eq!(
            expand("Towers Controlled: %2327w", &states),
            "Towers Controlled: 3"
        );
        assert_eq!(expand("%2327W/%2328w", &states), "3/1");
        assert_eq!(
            expand("Bases: %1779w  Resources: %1776w/%1780w", &states),
            "Bases: 0  Resources: 0/0",
            "an un-received state reads 0, never blank"
        );
        assert_eq!(
            expand("Graveyards Assaulted", &states),
            "Graveyards Assaulted"
        );
        assert_eq!(expand("", &states), "");
    }

    #[test]
    fn a_top_bit_value_prints_negative() {
        let mut states = WorldStates::default();
        states.write(&[(2327, 0xFFFF_FFFF)]);
        assert_eq!(expand("%2327w", &states), "-1");
    }

    /// Unreachable on shipped data.
    #[test]
    fn a_malformed_macro_emits_a_literal_percent() {
        let states = WorldStates::default();
        assert_eq!(expand("100% done", &states), "100% done");
        assert_eq!(expand("%d", &states), "%d");
        assert_eq!(expand("%", &states), "%");
        assert_eq!(
            expand("%123x", &states),
            "%x",
            "the digits scanned ahead of the bad letter are consumed either way"
        );
    }

    #[test]
    fn the_output_is_capped_at_the_reference_buffer() {
        let states = WorldStates::default();
        let long = "a".repeat(600);
        assert_eq!(expand(&long, &states).len(), 0xFF);
    }

    #[test]
    fn the_builder_gates_admit_the_right_rows() {
        // Eastern Plaguelands: map 0, area 139.
        assert!(admits(&row(0, 139, 0), 0, 139, false));
        assert!(!admits(&row(0, 139, 0), 0, 1377, false), "wrong area");
        assert!(!admits(&row(0, 139, 0), 1, 139, false), "wrong map");
        assert!(admits(&row(0, 0, 0), 0, 139, false), "area 0 is a wildcard");
        assert!(
            admits(&row(u32::MAX, 139, 0), 5, 139, false),
            "map -1 is a wildcard"
        );

        assert!(admits(&row(0, 139, 1), 0, 139, true));
        assert!(
            !admits(&row(0, 139, 1), 0, 139, false),
            "the world-PvP rows need the zone-defense channel"
        );
        for joined in [true, false] {
            assert!(
                !admits(&row(0, 139, 2), 0, 139, joined),
                "a scoreboard column is never an always-up row"
            );
        }
    }

    #[test]
    fn nothing_shows_before_the_first_init() {
        let catalog = WorldStateUiCatalog::from_rows(vec![(136, row(0, 139, 0))]);
        let states = WorldStates::default();
        assert!(build(&catalog, &states, true).is_empty());
    }

    #[test]
    fn a_row_without_a_state_variable_reads_one() {
        let states = WorldStates::default();
        assert_eq!(resolve(&row(0, 0, 0), &states).ui_state, 1);

        let mut with_state = row(0, 0, 0);
        with_state.state_variable = 2339;
        assert_eq!(
            resolve(&with_state, &states).ui_state,
            0,
            "a state that HAS an id and reads 0 is 0"
        );
        let mut states = WorldStates::default();
        states.write(&[(2339, 1)]);
        assert_eq!(resolve(&with_state, &states).ui_state, 1);
    }

    #[test]
    fn the_extended_ui_ids_are_resolved_to_values() {
        let mut states = WorldStates::default();
        states.write(&[(2427, 42), (2428, 7)]);
        let mut r = row(0, 139, 1);
        r.extended_ui = "CAPTUREPOINT".into();
        r.extended_ui_state = [2427, 2428, 0];
        assert_eq!(resolve(&r, &states).extended_ui_state, [42, 7, 0]);
    }

    /// Against the install's table: Eastern Plaguelands with and without the defense channel.
    #[test]
    fn the_real_table_builds_the_eastern_plaguelands_readout() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("chain");
        let catalog =
            benilla_formats::load_world_state_ui_catalog(&mut chain).expect("WorldStateUI");

        let mut states = WorldStates::default();
        states.init_scope(0, 139); // the server scoping us to Eastern Plaguelands
        states.write(&[(2327, 3), (2328, 1), (2426, 1), (2427, 60), (2428, 40)]);

        assert!(
            build(&catalog, &states, false).is_empty(),
            "no zone-defense channel — the world-PvP rows are all Type 1 (the Type gate, the \
             least obvious of the gates here)"
        );

        let rows = build(&catalog, &states, true);
        assert_eq!(
            rows.len(),
            3,
            "the two tower counters plus the progress bar"
        );
        assert_eq!(rows[0].text, "Towers Controlled: 3");
        assert_eq!(rows[0].icon, "Interface\\WorldStateFrame\\AllianceTower");
        assert_eq!(rows[0].tooltip, "Alliance Towers Controlled");
        assert_eq!(rows[0].ui_state, 1, "no StateVariable of its own");
        assert_eq!(rows[1].text, "Towers Controlled: 1");
        assert_eq!(rows[1].icon, "Interface\\WorldStateFrame\\HordeTower");
        assert_eq!(rows[2].text, "Progress: 60");
        assert_eq!(rows[2].extended_ui, "CAPTUREPOINT");
        assert_eq!(rows[2].extended_ui_state, [60, 40, 0]);
        assert_eq!(rows[2].ui_state, 1, "state 2426 reads 1");

        // The server flips a tower.
        states.write(&[(2327, 2), (2328, 2)]);
        let rows = build(&catalog, &states, true);
        assert_eq!(rows[0].text, "Towers Controlled: 2");
        assert_eq!(rows[1].text, "Towers Controlled: 2");

        let mut elsewhere = WorldStates::default();
        elsewhere.init_scope(0, 12); // Elwynn Forest
        assert!(build(&catalog, &elsewhere, true).is_empty());

        // Warsong Gulch: `Type` 0, with dynamic flag icons.
        let mut wsg = WorldStates::default();
        wsg.init_scope(489, 0);
        wsg.write(&[(1581, 2), (1582, 1), (1601, 3), (2339, 1)]);
        let rows = build(&catalog, &wsg, false);
        assert_eq!(rows.len(), 2, "Type 0 needs no channel");
        assert_eq!(rows[0].text, "2/3");
        assert_eq!(
            rows[0].dynamic_icon,
            "Interface\\WorldStateFrame\\HordeFlag"
        );
        assert_eq!(rows[0].ui_state, 1, "state 2339 — the Horde flag is up");
        assert_eq!(rows[1].text, "1/3");
        assert_eq!(rows[1].ui_state, 0, "state 2338 — the Alliance flag is not");
    }

    /// Against the install's `ChatChannels.dbc`: the one qualifying row is auto-joined, and
    /// `WorldDefense` (`DEFENSE` without `ZONE_DEP`) is the control.
    #[test]
    fn the_gates_channel_is_one_the_client_joins_by_itself() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("chain");
        let channels = benilla_formats::load_chat_channels_catalog(&mut chain).expect("channels");
        const REQUIRED: u32 = chan::ZONE_DEP | chan::DEFENSE;

        let qualifying: Vec<_> = channels
            .rows()
            .iter()
            .filter(|r| r.flags & REQUIRED == REQUIRED)
            .collect();
        assert_eq!(
            qualifying.len(),
            1,
            "one row opens the gate: {qualifying:?}"
        );
        assert_eq!(qualifying[0].id, 22);
        assert_eq!(qualifying[0].pattern, "LocalDefense - %s");
        assert!(
            qualifying[0].is_auto_join(),
            "and the client joins it unasked — otherwise the readout would never appear"
        );

        let world_defense = channels
            .rows()
            .iter()
            .find(|r| r.id == 23)
            .expect("WorldDefense");
        assert_ne!(
            world_defense.flags & REQUIRED,
            REQUIRED,
            "the realm-wide defense channel carries DEFENSE without ZONE_DEP and must NOT qualify"
        );
    }

    /// `0x49bd50` counts a record with a number (`0x49bd72`) in state 0 (`0x49bd7c`), and reads its
    /// row through its `+0x94` id (`0x49bd83`): a pending or suspended LocalDefense, a freed record
    /// and a custom channel of that name are none, and any name on row 22 is one.
    #[test]
    fn the_defense_gate_counts_a_confirmed_slot_by_its_row_id() {
        use crate::ui_chat::edit::{ChannelSlot, SlotRecord, SlotState};
        let joined = |slot: SlotRecord| crate::ui_chat::ChannelState {
            joined: vec![slot],
            channels: benilla_formats::ChatChannelsCatalog::from_rows(vec![
                benilla_formats::ChatChannelRow {
                    id: 22,
                    flags: 0x1_0003,
                    pattern: "LocalDefense - %s".into(),
                    shortcut: "LocalDefense".into(),
                },
            ]),
            ..Default::default()
        };
        let name = "LocalDefense - Elwynn Forest";
        let suspended = ChannelSlot {
            state: SlotState::Suspended,
            ..ChannelSlot::confirmed(name, 22)
        };
        for (slot, want) in [
            (ChannelSlot::confirmed(name, 22).into(), true),
            (ChannelSlot::confirmed("Anything", 22).into(), true),
            (ChannelSlot::pending(name, 22).into(), false),
            (suspended.into(), false),
            (ChannelSlot::confirmed(name, 0).into(), false),
            (SlotRecord::Freed { id: 22 }, false),
        ] {
            let label = format!("{slot:?}");
            let mut channels = joined(slot);
            channels.recompute_defense_channel();
            assert_eq!(channels.defense_channel, want, "{label}");
        }
    }

    /// The flag `0x49bd50` stores (`0xb6e5dc`) moves only at a confirm (`0x49bbbf`) or a free
    /// (`0x49bc41`): the walk's rename (state 2) and a pending join keep the defense rows up until
    /// a notice confirms or frees a slot.
    #[test]
    fn the_defense_rows_hold_through_a_rename_until_a_confirm_or_a_free() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.insert_non_send_resource(benilla_ui::script::UiScript::new().expect("a VM"));
        world.init_resource::<WorldStateLane>();
        let mut states = WorldStates::default();
        states.init_scope(0, 139);
        world.insert_resource(states);
        world.insert_resource(WorldStateUiRes(WorldStateUiCatalog::from_rows(vec![(
            1,
            row(0, 139, 1),
        )])));
        let mut channels = crate::ui_chat::ChannelState {
            channels: benilla_formats::ChatChannelsCatalog::from_rows(vec![
                benilla_formats::ChatChannelRow {
                    id: 22,
                    flags: 0x1_0003,
                    pattern: "LocalDefense - %s".into(),
                    shortcut: "LocalDefense".into(),
                },
            ]),
            zone_mask: Some(0),
            ..Default::default()
        };
        let (east, west) = (
            "LocalDefense - Eastern Plaguelands",
            "LocalDefense - Western Plaguelands",
        );
        channels.claim_slot(east, 22);
        channels.confirm_slot(east);
        world.insert_resource(channels);
        let shown = |world: &mut World| -> i64 {
            world
                .run_system_once(feed_world_state_ui)
                .expect("the feed runs");
            world
                .non_send_resource::<benilla_ui::script::UiScript>()
                .eval("return GetNumWorldStateUI()")
                .expect("the count")
        };
        assert_eq!(shown(&mut world), 1);
        world
            .resource_mut::<crate::ui_chat::ChannelState>()
            .rename_slot(east, west);
        world
            .resource_mut::<crate::ui_chat::ChannelState>()
            .claim_slot("MyChan", 0);
        assert_eq!(shown(&mut world), 1, "a rename and a pending join keep it");
        world
            .resource_mut::<crate::ui_chat::ChannelState>()
            .confirm_slot(west);
        assert_eq!(shown(&mut world), 1, "the confirm recomputes: still joined");
        world
            .resource_mut::<crate::ui_chat::ChannelState>()
            .free_slot(west);
        assert_eq!(shown(&mut world), 0, "the free recomputes");
    }

    /// `0x48f9f6`-`0x48fa17`: every world-state packet signals `WORLD_MAP_UPDATE` and then
    /// `UPDATE_WORLD_STATES` from its handler, an init and an update alike, whether or not a row
    /// moved, so two packets in a frame signal twice, each pair in order.
    #[test]
    fn every_packet_signals_the_map_then_the_readout() {
        use benilla_protocol::SessionEvent;
        let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
        let script = benilla_ui::script::UiScript::new().expect("a VM");
        script
            .run(
                "heard = '' local f = CreateFrame('Frame') \
                 f:RegisterEvent('UPDATE_WORLD_STATES') f:RegisterEvent('WORLD_MAP_UPDATE') \
                 f:SetScript('OnEvent', function() heard = heard .. event .. ' ' end)",
            )
            .expect("a listener");
        app.world_mut().insert_non_send_resource(script);
        let heard = |app: &mut App| -> String {
            app.world_mut().flush();
            crate::lanes::fire_in_order(app.world_mut());
            crate::script_calls::with_hosted_vm(app.world_mut(), |vm| {
                vm.expect("a VM")
                    .eval::<String>("local h = heard heard = '' return h")
            })
            .expect("the read")
        };
        let init = SessionEvent::WorldStates {
            scope: Some((0, 139)),
            states: vec![(2327, 3)],
        };
        let update = SessionEvent::WorldStates {
            scope: None,
            states: vec![(9999, 1)],
        };
        let pair = "WORLD_MAP_UPDATE UPDATE_WORLD_STATES ";
        crate::net::handlers::dispatch(app.world_mut(), vec![init.clone()]);
        assert_eq!(heard(&mut app), pair, "an init");
        assert_eq!(heard(&mut app), "", "a frame with no packet");
        crate::net::handlers::dispatch(app.world_mut(), vec![init, update]);
        assert_eq!(
            heard(&mut app),
            format!("{pair}{pair}"),
            "a re-sent init and an update in one frame signal once each"
        );
    }

    /// The defense flip signals the readout alone (`0x49bdd6`).
    #[test]
    fn a_readout_signal_fires_without_the_map() {
        let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
        let script = benilla_ui::script::UiScript::new().expect("a VM");
        script
            .run(
                "heard = '' local f = CreateFrame('Frame') \
                 f:RegisterEvent('UPDATE_WORLD_STATES') f:RegisterEvent('WORLD_MAP_UPDATE') \
                 f:SetScript('OnEvent', function() heard = heard .. event .. ' ' end)",
            )
            .expect("a listener");
        app.world_mut().insert_non_send_resource(script);
        app.world_mut()
            .resource_mut::<WorldStateLane>()
            .push(WorldStateSignal::Readout);
        crate::lanes::fire_in_order(app.world_mut());
        let heard = crate::script_calls::with_hosted_vm(app.world_mut(), |vm| {
            vm.expect("a VM").eval::<String>("return heard")
        })
        .expect("the read");
        assert_eq!(heard, "UPDATE_WORLD_STATES ");
    }

    /// An init clears the table.
    #[test]
    fn an_init_forgets_the_previous_zone() {
        let mut states = WorldStates::default();
        states.init_scope(0, 139);
        states.write(&[(2327, 3)]);
        assert_eq!(states.get(2327), 3);

        states.init_scope(0, 12);
        assert_eq!(states.get(2327), 0, "the previous zone's key is gone");
        assert_eq!(states.scope(), Some((0, 12)));
    }
}
