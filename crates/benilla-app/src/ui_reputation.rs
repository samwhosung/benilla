//! The reputation pane's feed: the player's wire reputation slots resolved against `Faction.dbc`,
//! pushed as one snapshot with `UPDATE_FACTION` on a change, and the pane's three verbs sent.
//!
//! A faction is a row when it has a reputation slot and one of its four race/class mask slots
//! fits the player; the client adds it inside the accept block that picks the base value
//! (`0x4d5555`). The pane lists only rows with `VISIBLE`, which `SMSG_SET_FACTION_VISIBLE` sets,
//! but unlisted rows are pushed too: they carry the header names, and only one of the five header
//! factions is `VISIBLE`. Flag `0x08` is `HEADER`, not "force invisible".
//!
//! The wire standing excludes the race/class base, so the pane's total is base plus wire: vmangos
//! stores `standing - BaseRep` (`ReputationMgr.cpp:261`) and reports the sum
//! (`ReputationMgr.cpp:82`). The rank is [`benilla_formats::reputation_rank`], shared with the
//! unit-reaction decode so the pane and the nameplate agree.

use bevy::prelude::*;

use benilla_ui::script::{FactionEntry, ReputationSend, ReputationState, UiScript};

use crate::net::{ClientCommand, NetCommands, ObjectStore, Reputations, SelfPlayer};
use crate::target::Factions;
use crate::ui_script::{UiFeed, UiInput};

/// Rank `r` spans `RANK_BOUNDS[r]..RANK_BOUNDS[r + 1]`, the client's table at `0x80928c`: vmangos's
/// `PointsInRank` walked up from -42000 (`ReputationMgr.cpp:30`), the edges
/// [`benilla_formats::reputation_rank`] ranks by. The Lua `standingID` is the rank plus one.
const RANK_BOUNDS: [i32; 9] = [-42000, -6000, -3000, 0, 3000, 9000, 21000, 42000, 43000];

/// The reputation pane's feed and outbound drain; the bindings are `benilla-ui`'s.
pub(crate) struct UiReputationPlugin;

impl Plugin for UiReputationPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                feed_reputation.in_set(UiFeed),
                drain_reputation_sends.after(UiInput),
            ),
        );
    }
}

/// One faction's row, or `None` when it is not in the player's list at all; a row the pane does
/// not draw is still a row, with `visible: false`.
pub(crate) fn reputation_row(
    faction_id: u32,
    info: &benilla_formats::FactionInfo,
    catalog: &benilla_formats::FactionCatalog,
    flags: u8,
    wire_standing: i32,
    race: u8,
    class: u8,
) -> Option<FactionEntry> {
    use benilla_formats::faction_flags as flag;
    // The membership gate: no fitting race/class slot, no row at all (`0x4d5555`).
    info.slot_for(race, class)?;
    let standing = info.base_for(race, class) + wire_standing;
    let rank = benilla_formats::reputation_rank(standing);
    Some(FactionEntry {
        faction_id,
        rep_list_id: u32::try_from(info.rep_index).ok()?,
        parent_id: info.team,
        name: catalog.faction_name(faction_id)?.to_string(),
        description: catalog
            .faction_description(faction_id)
            .unwrap_or_default()
            .to_string(),
        standing,
        standing_id: rank + 1,
        bar_min: RANK_BOUNDS[rank as usize],
        bar_max: RANK_BOUNDS[rank as usize + 1],
        visible: flags & flag::VISIBLE != 0,
        is_header: flags & flag::HEADER != 0,
        at_war: flags & flag::AT_WAR != 0,
        // What `GetFactionInfo` reports; the toggle itself is stricter (`0x4d5fd0`).
        can_toggle_at_war: flags & flag::PEACE_FORCED == 0 && standing >= -3000,
        inactive: flags & flag::INACTIVE != 0,
    })
}

/// The by-value inputs of the feed's gate, `(race, class, watched)`; the other two are resources.
type ReputationInputs = Option<(u8, u8, Option<u32>)>;

/// Push the snapshot and fire `UPDATE_FACTION` when it differs. The build (54 rows of names and
/// descriptions) runs only when the wire slots, the catalog, race/class or the watched index
/// move, and the equality check behind it keeps a change to no listed row quiet. The first push
/// fires too: the stock watch bar initializes off `UPDATE_FACTION` at login.
fn feed_reputation(
    script: Option<NonSendMut<UiScript>>,
    self_store: Query<&ObjectStore, With<SelfPlayer>>,
    reputations: Res<Reputations>,
    factions: Option<Res<Factions>>,
    mut last: Local<crate::ui_script::VmMemo<Option<ReputationState>>>,
    mut last_inputs: Local<crate::ui_script::VmMemo<ReputationInputs>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let last = last.get(&script);
    let last_inputs = last_inputs.get(&script);
    let (Ok(store), Some(factions_res)) = (self_store.single(), factions.as_ref()) else {
        return;
    };
    let catalog = factions_res.catalog();
    let (race, class) = (
        store.0.unit_race().unwrap_or(0),
        store.0.unit_class().unwrap_or(0),
    );
    let watched = store
        .0
        .player_watched_faction()
        .and_then(|i| u32::try_from(i).ok());

    let inputs = (race, class, watched);
    let moved = reputations.is_changed()
        || factions_res.is_changed()
        || last_inputs.as_ref() != Some(&inputs)
        || last.is_none();
    if !moved {
        return;
    }
    *last_inputs = Some(inputs);

    let mut entries = Vec::new();
    for (faction_id, info) in catalog.reputation_factions() {
        // An uncovered slot reads `(0, 0)`, unmet, and its row stays: a header's name rides on it.
        let (flags, wire) = usize::try_from(info.rep_index)
            .ok()
            .and_then(|i| reputations.0.get(i))
            .copied()
            .unwrap_or((0, 0));
        if let Some(row) = reputation_row(faction_id, info, catalog, flags, wire, race, class) {
            entries.push(row);
        }
    }
    // Sorted before the equality check, so a map reshuffle is not a change.
    entries.sort_by_key(|e| e.faction_id);

    let fresh = ReputationState { entries, watched };
    if last.as_ref() == Some(&fresh) {
        return;
    }
    script.set_reputation(fresh.clone());
    *last = Some(fresh);
    script.fire_event("UPDATE_FACTION", vec![]);
}

/// Send the pane's queued verbs; none is acked, and the engine already holds its own copy.
fn drain_reputation_sends(script: Option<NonSendMut<UiScript>>, commands: Res<NetCommands>) {
    let Some(mut script) = script else {
        return;
    };
    for send in script.take_reputation_sends() {
        let cmd = match send {
            ReputationSend::AtWar {
                rep_list_id,
                at_war,
            } => ClientCommand::SetFactionAtWar {
                rep_list_id,
                at_war,
            },
            ReputationSend::Inactive {
                rep_list_id,
                inactive,
            } => ClientCommand::SetFactionInactive {
                rep_list_id,
                inactive,
            },
            // No watch is -1 on the wire: slot 0 is the Bloodsail Buccaneers.
            ReputationSend::Watch(slot) => ClientCommand::SetWatchedFaction {
                rep_list_id: slot.map_or(benilla_protocol::messages::WATCHED_FACTION_NONE, |s| {
                    s as i32
                }),
            },
        };
        let _ = commands.0.send(cmd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pane's bar reads [`RANK_BOUNDS`], the nameplate the rank function: they must agree.
    #[test]
    fn rank_bounds_agree_with_the_rank_function() {
        for rank in 0u8..=7 {
            let (floor, ceiling) = (RANK_BOUNDS[rank as usize], RANK_BOUNDS[rank as usize + 1]);
            assert_eq!(
                benilla_formats::reputation_rank(floor),
                rank,
                "rank {rank}'s floor {floor}"
            );
            assert_eq!(
                benilla_formats::reputation_rank(ceiling - 1),
                rank,
                "rank {rank}'s top {}",
                ceiling - 1
            );
            if rank > 0 {
                assert_eq!(
                    benilla_formats::reputation_rank(floor - 1),
                    rank - 1,
                    "one below rank {rank}'s floor"
                );
            }
        }
        // vmangos's `PointsInRank` (`ReputationMgr.cpp:30`).
        let widths: Vec<i32> = RANK_BOUNDS.windows(2).map(|w| w[1] - w[0]).collect();
        assert_eq!(widths, [36000, 3000, 3000, 3000, 6000, 12000, 21000, 1000]);
    }

    /// Stormwind (72) has four race-gated base slots, `0x4c` 3100, `0xb2` (Horde) -42000, `0x01`
    /// (Human) 4000 and an empty one, so a human starts at 4000, Friendly, from the third slot.
    /// Skips without client data.
    #[test]
    fn real_stormwind_starts_friendly_for_a_human_and_hidden_factions_never_list() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let cat = benilla_formats::load_faction_catalog(&mut chain).expect("factions");
        use benilla_formats::faction_flags as flag;
        // A human warrior (race 1, class 1) who has met Stormwind and gained nothing since.
        let sw = cat.reputation_faction(72).expect("Stormwind");
        let row = reputation_row(72, sw, &cat, flag::VISIBLE | flag::PEACE_FORCED, 0, 1, 1)
            .expect("Stormwind lists");
        assert_eq!(row.name, "Stormwind");
        assert_eq!(
            row.standing, 4000,
            "the human-gated base slot, with no wire gain on top"
        );
        assert_eq!(row.standing_id, 5, "FACTION_STANDING_LABEL5 = Friendly");
        assert_eq!((row.bar_min, row.bar_max), (3000, 9000));
        assert_eq!(row.parent_id, 469, "grouped under Alliance");
        assert!(
            !row.can_toggle_at_war,
            "PEACE_FORCED: you cannot go to war with your own people"
        );
        // A wire gain rides on top of the base.
        let honored = reputation_row(72, sw, &cat, flag::VISIBLE, 5000, 1, 1).expect("lists");
        assert_eq!(honored.standing, 9000);
        assert_eq!(honored.standing_id, 6, "Honored begins exactly at 9000");
        assert_eq!((honored.bar_min, honored.bar_max), (9000, 21000));

        // The same empty wire slot, read by an orc: the Horde-gated slot.
        let orc = reputation_row(72, sw, &cat, flag::VISIBLE, 0, 2, 1).expect("lists");
        assert_eq!(orc.standing, -42000, "the Horde race mask's base");
        assert_eq!(orc.standing_id, 1, "FACTION_STANDING_LABEL1 = Hated");

        // Unmet is not visible, not absent: the row comes back with its name.
        let unmet = reputation_row(72, sw, &cat, 0, 0, 1, 1).unwrap();
        assert!(!unmet.visible, "unmet");
        assert_eq!(unmet.name, "Stormwind", "and still carries its name");
        // `HIDDEN` suppresses the auto-reveal and the rank-change chat line, never the row.
        let hidden = reputation_row(72, sw, &cat, flag::VISIBLE | flag::HIDDEN, 0, 1, 1).unwrap();
        assert!(hidden.visible, "HIDDEN is not a list gate");
        assert!(!hidden.is_header, "and it is not the header bit either");
    }

    /// On 1.12's data the membership gate (`0x4d5555`) excludes only Cenarion Circle (609) for
    /// druids of the six races that cannot be druids. Its first slot takes every race with class
    /// mask `0x1df`, no druids, and its second Night Elf and Tauren druids at 2000, so a rule that
    /// stopped at the first match would give a Night Elf druid 0. Skips without client data.
    #[test]
    fn real_membership_gate_excludes_only_druids_of_the_six_non_druid_races() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let cat = benilla_formats::load_faction_catalog(&mut chain).expect("factions");

        // 1.12 has no class 6 and no class 10; druid is 11.
        const CLASSES: [u8; 9] = [1, 2, 3, 4, 5, 7, 8, 9, 11];
        const DRUID: u8 = 11;
        let mut excluded: Vec<(u32, u8, u8)> = Vec::new();
        for race in 1..=8u8 {
            for class in CLASSES {
                for (id, info) in cat.reputation_factions() {
                    if info.slot_for(race, class).is_none() {
                        excluded.push((id, race, class));
                    }
                }
            }
        }
        let expected: Vec<(u32, u8, u8)> = [1u8, 2, 3, 5, 7, 8]
            .into_iter()
            .map(|race| (609, race, DRUID))
            .collect();
        excluded.sort_unstable();
        assert_eq!(
            excluded, expected,
            "the gate should exclude only Cenarion Circle from druids of the six races that \
             cannot be druids — i.e. nothing a player can roll"
        );

        // Last match wins, and the druid slot is second.
        let cc = cat.reputation_faction(609).expect("Cenarion Circle");
        assert_eq!(
            cc.slot_for(4, DRUID),
            Some(1),
            "a Night Elf druid takes slot 1"
        );
        assert_eq!(cc.base_for(4, DRUID), 2000);
        assert_eq!(cc.slot_for(6, DRUID), Some(1), "so does a Tauren druid");
        assert_eq!(cc.slot_for(1, 1), Some(0), "a human warrior takes slot 0");
        assert_eq!(cc.base_for(1, 1), 0);
    }

    /// All five header factions carry `HEADER` and four lack `VISIBLE`, so filtering on visibility
    /// would leave the headers blank. The flags are the DBC defaults a human warrior is seeded with
    /// (`ReputationMgr::GetDefaultStateFlags`). Skips without client data.
    #[test]
    fn real_header_factions_carry_the_header_bit_and_still_reach_the_engine() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let cat = benilla_formats::load_faction_catalog(&mut chain).expect("factions");
        use benilla_formats::faction_flags as flag;

        // Every faction some reputation faction names as its `team`: the pane's headers.
        let parents: std::collections::BTreeSet<u32> = cat
            .reputation_factions()
            .map(|(_, f)| f.team)
            .filter(|&t| t != 0)
            .collect();
        assert_eq!(
            parents.iter().copied().collect::<Vec<_>>(),
            [67, 169, 469, 891, 892],
            "Horde, Steamwheedle, Alliance, and the two battleground blocs"
        );

        for id in parents {
            let info = cat
                .reputation_faction(id)
                .unwrap_or_else(|| panic!("parent {id} has its own reputation slot"));
            let flags = u8::try_from(info.default_flags_for(1, 1)).expect("a flag byte");
            assert!(
                flags & flag::HEADER != 0,
                "parent {id} must carry the HEADER bit; got {flags:#04x}"
            );
            let row = reputation_row(id, info, &cat, flags, 0, 1, 1)
                .unwrap_or_else(|| panic!("parent {id} still resolves"));
            assert!(row.is_header, "parent {id} is a header row");
            assert!(
                !row.name.is_empty(),
                "parent {id} reaches the engine WITH its name — that name is the header"
            );
        }
        // Only one of the five is also `VISIBLE`.
        let visible_parents = [67u32, 169, 469, 891, 892]
            .into_iter()
            .filter(|&id| {
                let info = cat.reputation_faction(id).unwrap();
                u8::try_from(info.default_flags_for(1, 1)).unwrap() & flag::VISIBLE != 0
            })
            .count();
        assert_eq!(visible_parents, 1, "only Alliance carries VISIBLE as well");
    }
}

#[cfg(test)]
mod live_wire_tests {
    use super::*;
    use benilla_ui::script::UiScript;

    /// The `SMSG_INITIALIZE_FACTIONS` flag bytes live vmangos sends a fresh level-1 human warrior
    /// (the `faction_probe` example); every other slot's flags and all 64 standings are 0.
    const LIVE_FLAGS: &[(usize, u8)] = &[
        (0, 0x02),
        (2, 0x02),
        (3, 0x02),
        (4, 0x10),
        (6, 0x02),
        (8, 0x10),
        (10, 0x08),
        (11, 0x09),
        (12, 0x0e),
        (14, 0x06),
        (15, 0x06),
        (16, 0x06),
        (17, 0x06),
        (18, 0x11),
        (19, 0x11),
        (20, 0x11),
        (21, 0x11),
        (22, 0x04),
        (23, 0x04),
        (24, 0x04),
        (25, 0x04),
        (26, 0x04),
        (29, 0x04),
        (30, 0x04),
        (31, 0x04),
        (32, 0x04),
        (33, 0x04),
        (34, 0x04),
        (35, 0x02),
        (38, 0x02),
        (39, 0x14),
        (40, 0x10),
        (41, 0x02),
        (43, 0x10),
        (44, 0x10),
        (45, 0x10),
        (46, 0x06),
        (47, 0x18),
        (48, 0x0e),
        (50, 0x10),
        (51, 0x10),
        (52, 0x02),
        (53, 0x10),
        (54, 0x02),
    ];

    fn live_store() -> Vec<(u8, i32)> {
        let mut slots = vec![(0u8, 0i32); 64];
        for &(i, flags) in LIVE_FLAGS {
            slots[i].0 = flags;
        }
        slots
    }

    /// A new human warrior's Reputation tab shows the Alliance header (`0x09`, a header that is
    /// also visible) over its four cities, and nothing else. Skips without client data.
    #[test]
    fn a_fresh_alliance_characters_pane_off_live_wire_bytes() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let cat = benilla_formats::load_faction_catalog(&mut chain).expect("factions");
        let store = live_store();

        // The live server marks exactly the five parents with `HEADER`.
        use benilla_formats::faction_flags as flag;
        let headers: Vec<usize> = LIVE_FLAGS
            .iter()
            .filter(|(_, f)| f & flag::HEADER != 0)
            .map(|&(i, _)| i)
            .collect();
        assert_eq!(
            headers,
            [10, 11, 12, 47, 48],
            "the live server marks exactly the five parent factions as headers"
        );

        // The feed, for a human warrior (race 1, class 1).
        let mut entries = Vec::new();
        for (id, info) in cat.reputation_factions() {
            let (flags, wire) = usize::try_from(info.rep_index)
                .ok()
                .and_then(|i| store.get(i))
                .copied()
                .unwrap_or((0, 0));
            if let Some(row) = reputation_row(id, info, &cat, flags, wire, 1, 1) {
                entries.push(row);
            }
        }
        entries.sort_by_key(|e| e.faction_id);
        assert_eq!(
            entries.len(),
            54,
            "every reputation faction reaches the engine"
        );
        assert_eq!(
            entries.iter().filter(|e| e.visible).count(),
            5,
            "and exactly five of them are ones this character has met"
        );

        // The engine's tree.
        let mut s = UiScript::new().expect("VM");
        s.set_reputation(ReputationState {
            entries,
            watched: None,
        });
        let rows: Vec<String> = s
            .eval(
                "local t = {} for i = 1, GetNumFactions() do t[i] = (GetFactionInfo(i)) end return t",
            )
            .expect("rows");
        assert_eq!(
            rows,
            [
                "Alliance",
                "Darnassus",
                "Gnomeregan Exiles",
                "Ironforge",
                "Stormwind",
            ],
            "the Alliance header and its four cities — Alliance is the HEADER, not a fifth bar"
        );

        // The bars read their DBC bases; the capture has no standing.
        let (name, sid, min, max, val) = s
            .eval::<(String, i64, i64, i64, i64)>(
                "local n,_,s,mn,mx,v = GetFactionInfo(5) return n,s,mn,mx,v",
            )
            .unwrap();
        assert_eq!(name, "Stormwind");
        assert_eq!(val, 4000, "the human-gated base, with nothing gained yet");
        assert_eq!(sid, 5, "Friendly");
        assert_eq!((min, max), (3000, 9000));
        assert!(s.errors().is_empty(), "{:#?}", s.errors());
    }
}
