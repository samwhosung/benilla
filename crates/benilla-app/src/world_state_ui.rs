//! The always-up world-state readout (`WorldStateFrame`) — the app half behind
//! stock `Interface\FrameXML\WorldStateFrame.xml` and benilla-ui's `script/worldstate.rs` bindings.
//!
//! This is report **B190**'s second half: the alliance↔horde progress UI. `WorldStateUI.dbc` says
//! which world states are *displayed*, where, and with what label; [`crate::world_state`] holds
//! what the server actually sent; this module joins them and pushes the result.
//!
//! Everything below is the reference's.
//!
//! ## The list builder (`0x4c56e0`)
//!
//! Walks the whole DBC and admits a row on three gates, each failure skipping the row rather than
//! aborting the walk:
//!
//! 1. **Map** — `MapID == -1` (a wildcard with *zero* shipped rows) or `MapID` equals the scope's
//!    map;
//! 2. **Area** — `AreaID == 0` (wildcard) or equal to the scope's area;
//! 3. **Type** — `0` accepts unconditionally, `1` accepts only while [`defense_channel_joined`],
//!    `2` (the battleground scoreboard columns) always rejects.
//!
//! **The scope is the server's last `SMSG_INIT_WORLD_STATES`, not the player's position**
//! ([`crate::world_state::WorldStates::scope`]). That is not a simplification: the two globals it
//! mirrors have exactly two writers each image-wide, the init clear and the logout reset, so a
//! plain zone change moves nothing here. It matters — walk into Eastern Plaguelands and the readout
//! appears when the *server* says so, which is also when the states it would read arrive.
//!
//! ## Why the towers need a chat channel
//!
//! The `Type == 1` gate is the surprising one, and it is the whole of what the world-PvP rows
//! (Eastern Plaguelands' three, Silithus's two) hang on. The reference recomputes a flag by
//! scanning the channels the player has **joined** and asking whether any of their
//! `ChatChannels.dbc` rows carries **both** `ZONE_DEP` and `DEFENSE` — which in the shipped table
//! is row 22 alone, the zone-scoped defense channel. Leave that channel and the tower readout
//! disappears; the map icons, which are gated on the world states themselves, stay.
//!
//! ## The text (`0x508560`)
//!
//! Exactly one of the ten returned values is expanded, and by an expander that is **not** the
//! NPC-text one — different function, different grammar, sharing only the table getter. See
//! [`expand`].

use bevy::ecs::system::NonSendMut;
use bevy::prelude::*;

use benilla_formats::{chat_channel_flags as chan, WorldStateUiCatalog, WorldStateUiRow};
use benilla_ui::script::{UiScript, WorldStateUiView};

use benilla_assets::{AssetSet, LockRecover, WorldAssets};

use crate::world_state::WorldStates;

/// The shared `WorldStateUI.dbc` catalog. Absent if the DBC failed to read, which degrades to an
/// empty readout rather than an error.
#[derive(Resource)]
pub(crate) struct WorldStateUiRes(pub(crate) WorldStateUiCatalog);

/// Startup: load the table off the patch chain.
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

/// The `Type == 1` gate: has the player joined a **zone-dependent defense** channel?
///
/// The reference scans its joined-channel array and tests each channel's `ChatChannels.dbc`
/// `Flags` for `0x2 | 0x10000` (`0x49bd9b`/`0x49bda2`) — both bits, not either. Exactly one shipped
/// row qualifies: id 22, flags `0x010003`. Our joined list is names, so each resolves back to its
/// row the same way the server resolves it (`row_for_name`); a custom channel resolves to nothing
/// and cannot open the gate, which is right.
fn defense_channel_joined(channels: &crate::ui_chat::ChannelState) -> bool {
    const REQUIRED: u32 = chan::ZONE_DEP | chan::DEFENSE;
    channels
        .iter_names()
        .filter_map(|name| channels.channels.row_for_name(name))
        .any(|row| row.flags & REQUIRED == REQUIRED)
}

/// Expand a `WorldStateUI` label — the reference's `0x508560`, and **not** the NPC-text expander
/// ([`crate::npc_text`]). The two share only the world-state getter: every `call` in this one
/// resolves to a CRT helper, the getter, or the string primitives, and the quest-text token
/// handler `0x5070a0` is not among them.
///
/// The grammar is `%<digits>W|w` **and nothing else** — no `$` sigil, and no `e` (negated-key)
/// form. A hit reads the table at the parsed id and prints the value through `"%d"`. Anything else
/// after a `%` emits a literal `"%"`, sets an error flag the caller ignores, and **leaves the
/// offending character unconsumed** so it falls through as ordinary text — the digits scanned ahead
/// of it are consumed either way. No shipped row reaches that leg (all nine macro-bearing rows are
/// well-formed), so it is reproduced rather than relied upon.
///
/// The reference writes into a 260-byte buffer with a 256-byte limit, so a label longer than that
/// is truncated. Kept, because a UI string silently growing past what the reference would show is
/// a difference the frame would render.
fn expand(text: &str, states: &WorldStates) -> String {
    /// The reference's `strncat` bound — `0x508560`'s caller passes `0x100`.
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
                // `atoi` over the run — an empty run is 0, which reads the table at key 0.
                let id: u32 = text[digits_at..j].parse().unwrap_or(0);
                push(&states.get(id).to_string(), &mut out);
                i = j + 1;
            }
            // The error leg: a literal `%`, and the letter is left for the next pass.
            _ => {
                push("%", &mut out);
                i = j;
            }
        }
    }
    out
}

/// Build the rows for one scope — the builder's gates, then `GetWorldStateUIInfo`'s resolution of
/// each admitted row into the ten values it answers with.
fn build(
    catalog: &WorldStateUiCatalog,
    states: &WorldStates,
    defense_channel: bool,
) -> Vec<WorldStateUiView> {
    let Some((map, area)) = states.scope() else {
        // No init seen. The reference's rebuild trigger refuses to run before one arrives, and its
        // filter globals read `-1`, which no row matches.
        return Vec::new();
    };
    catalog
        .rows()
        .filter(|(_, row)| admits(row, map, area, defense_channel))
        .map(|(_, row)| resolve(row, states))
        .collect()
}

/// The three gates, in the builder's order (see the module doc).
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

/// `GetWorldStateUIInfo`'s ten values for one row (`0x4c5a70`). Only [`WorldStateUiRow::text`] is
/// expanded; every other string is the DBC column verbatim, and the three extended ids answer
/// their *resolved values*.
fn resolve(row: &WorldStateUiRow, states: &WorldStates) -> WorldStateUiView {
    WorldStateUiView {
        // A row with no `StateVariable` answers the constant 1, not 0 — the miss leg is a literal
        // `1.0` at `0x4c5ad8`, so "no state of its own" reads as on.
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

/// Push the readout when any of its inputs moves. The setter diffs before firing
/// `UPDATE_WORLD_STATES`, so a frame in which nothing changed costs one table walk and no event.
fn feed_world_state_ui(
    script: Option<NonSendMut<UiScript>>,
    catalog: Option<Res<WorldStateUiRes>>,
    states: Res<WorldStates>,
    channels: Res<crate::ui_chat::ChannelState>,
    mut defense_channel: Local<bool>,
    mut last: Local<crate::ui_script::VmMemo<Option<(u64, bool)>>>,
) {
    // Recomputed only when the roster moves. The scan resolves each joined NAME back to its DBC
    // row, which allocates per row — cheap once a login, wasteful sixty times a second. A system
    // that has never run sees every resource as changed, so the first frame computes it.
    if channels.is_changed() {
        *defense_channel = defense_channel_joined(&channels);
    }
    let defense_channel = *defense_channel;
    let (Some(mut script), Some(catalog)) = (script, catalog) else {
        return;
    };
    // The reference's own two rebuild triggers: a world-state packet (which is also the only thing
    // that moves the scope) and the defense-channel flag flipping.
    let key = (states.generation(), defense_channel);
    if *last.get(&script) == Some(key) {
        return;
    }
    *last.get(&script) = Some(key);
    script.set_world_state_ui(build(&catalog.0, &states, defense_channel));
}

pub(crate) struct WorldStateUiPlugin;

impl Plugin for WorldStateUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, load_world_state_ui.after(AssetSet::Open))
            // After the script tick, like every other feed: the queued event dispatches next tick.
            .add_systems(Update, feed_world_state_ui.after(crate::ui_script::UiInput));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// The `%<digits>w` grammar: a hit prints the table value through `%d`, a miss prints `0`
    /// (the getter's own miss leg), and case does not matter.
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

    /// A value with the top bit set prints negative — the `%d` width, same as `$<n>w`.
    #[test]
    fn a_top_bit_value_prints_negative() {
        let mut states = WorldStates::default();
        states.write(&[(2327, 0xFFFF_FFFF)]);
        assert_eq!(expand("%2327w", &states), "-1");
    }

    /// The error leg: a `%` not followed by an optional digit run and a `w` emits a literal `%`
    /// and leaves the offending character to fall through as text. It is unreachable on shipped
    /// data; it is here so the behaviour is decided rather than accidental.
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

    /// The reference's 256-byte buffer bound.
    #[test]
    fn the_output_is_capped_at_the_reference_buffer() {
        let states = WorldStates::default();
        let long = "a".repeat(600);
        assert_eq!(expand(&long, &states).len(), 0xFF);
    }

    /// The three builder gates. Map `-1` and area `0` are wildcards; `Type` 2 never enters this
    /// list at all, and `Type` 1 waits for the defense channel.
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

        // Type: 0 unconditional, 1 on the channel, 2 never.
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

    /// Before any `SMSG_INIT_WORLD_STATES` the readout is empty — the reference's filter globals
    /// read `-1` and its rebuild trigger will not run at all.
    #[test]
    fn nothing_shows_before_the_first_init() {
        let catalog = WorldStateUiCatalog::from_rows(vec![(136, row(0, 139, 0))]);
        let states = WorldStates::default();
        assert!(build(&catalog, &states, true).is_empty());
    }

    /// A row with no `StateVariable` answers `uiState = 1`, not `0` — the miss leg is a literal
    /// `1.0`, so "no state of its own" reads as on rather than off.
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

    /// The extended-UI ids are answered as resolved *values*, not as the ids the DBC holds.
    #[test]
    fn the_extended_ui_ids_are_resolved_to_values() {
        let mut states = WorldStates::default();
        states.write(&[(2427, 42), (2428, 7)]);
        let mut r = row(0, 139, 1);
        r.extended_ui = "CAPTUREPOINT".into();
        r.extended_ui_state = [2427, 2428, 0];
        assert_eq!(resolve(&r, &states).extended_ui_state, [42, 7, 0]);
    }

    /// The whole join, against the REAL table: in Eastern Plaguelands, with the zone-defense
    /// channel joined and the server's tower counts in, the readout is the two labelled tower rows
    /// plus the capture-point progress row — and without the channel it is empty. Skips without
    /// client data.
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
            "no zone-defense channel — the world-PvP rows are all Type 1 (this IS report B190's \
             second half, and its least obvious gate)"
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

        // The server flips a tower: the same rows, new numbers, no rebuild needed.
        states.write(&[(2327, 2), (2328, 2)]);
        let rows = build(&catalog, &states, true);
        assert_eq!(rows[0].text, "Towers Controlled: 2");
        assert_eq!(rows[1].text, "Towers Controlled: 2");

        // Elsewhere on the same continent, nothing — the area gate.
        let mut elsewhere = WorldStates::default();
        elsewhere.init_scope(0, 12); // Elwynn Forest
        assert!(build(&catalog, &elsewhere, true).is_empty());

        // Warsong Gulch: Type 0, so no channel needed, and its rows carry the dynamic flag icons.
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

    /// The `Type == 1` gate is only worth building if the channel it waits on is one the client
    /// actually joins — otherwise the whole Eastern Plaguelands readout ships dead. Against the
    /// real `ChatChannels.dbc`: exactly one row carries both `ZONE_DEP` and `DEFENSE`, it is
    /// `LocalDefense - %s`, and it is an auto-join row. `WorldDefense` is the control that could
    /// have made this pass for the wrong reason — it carries `DEFENSE` without `ZONE_DEP`, and
    /// must not open the gate. Skips without client data.
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

    /// An init CLEARS the table (the C2 correction): a state received in the previous zone must
    /// not still read through after the server re-scopes us.
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
