//! The reputation pane's app half: resolve the player's wire reputation slots against
//! `Faction.dbc`, push the flat snapshot into the VM's reputation seam
//! ([`benilla_ui::script::ReputationState`]), fire `UPDATE_FACTION` on a change, and drain the
//! pane's three outbound verbs back onto the wire.
//!
//! The seam's module doc (`benilla-ui`'s `script::reputation`) is the display law; this file is the
//! **data** law it consumes, and there are exactly three pieces of it:
//!
//! 1. **Which factions are rows.** A `Faction.dbc` row participates iff it has a reputation slot
//!    (`reputationIndex >= 0`) **and at least one of its four race/class mask slots fits the
//!    player** — the client's own membership gate at `0x4d5555`, where the add call sits inside the
//!    accept block of the same loop that picks the base value. Whether the pane then *lists* it is
//!    the single flag `VISIBLE`, off until the player first meets them — which is why
//!    `SMSG_SET_FACTION_VISIBLE` has to be applied (`net::session::reputation_visible`).
//!
//!    **Unlisted factions are pushed anyway, carrying `visible: false`.** They are how the pane's
//!    headers learn their names: all five header factions carry `HEADER` (`0x08`) and only one of
//!    them carries `VISIBLE` as well, so filtering on visibility would group the tree correctly and
//!    then label every header with an empty string.
//! 2. **The standing.** The wire standing EXCLUDES the DBC race/class base value; the total the
//!    pane ranks is `base_for(race, class) + wire`. Verified on the server side, which is the side
//!    that authors both numbers: vmangos stores `faction.Standing = standing - BaseRep`
//!    (`ReputationMgr::SetOneFactionReputation`) and reports
//!    `GetBaseReputation(entry) + state->Standing` (`GetReputation`).
//! 3. **The rank window.** [`benilla_formats::reputation_rank`] ranks the total on the 0..=7 scale
//!    (shared with the unit-reaction decode, so the pane and the nameplate can never disagree); the
//!    absolute floor/ceiling of that rank are [`RANK_BOUNDS`] — byte-for-byte the client's own
//!    `.rdata` table at `0x80928c`. The Lua tuple's `standingID` is the rank plus one.
//!
//! And one flag reading that is easy to get wrong and was: **`0x08` is `HEADER`, not
//! "force-invisible"** — see `benilla_formats::faction_flags`. `canToggleAtWar` is
//! `!PEACE_FORCED && standing >= -3000`; the client's toggle enforces more than it reports, but the
//! extra conditions (in combat, and the floor applying only toward peace) are the app's to add if
//! the pane ever needs them.

use bevy::prelude::*;

use benilla_ui::script::{FactionEntry, ReputationSend, ReputationState, UiScript};

use crate::net::{ClientCommand, NetCommands, ObjectStore, Reputations, SelfPlayer};
use crate::target::Factions;
use crate::ui_script::{UiFeed, UiInput};

/// The cumulative standing edges of the eight ranks: `RANK_BOUNDS[r]` is rank `r`'s absolute floor
/// and `RANK_BOUNDS[r + 1]` its ceiling. The widths between them are vmangos's `PointsInRank`
/// (36000, 3000, 3000, 3000, 6000, 12000, 21000, 1000) walked up from the −42000 floor, so this
/// table and [`benilla_formats::reputation_rank`]'s thresholds are the same numbers read two ways —
/// `rank_bounds_agree_with_the_rank_function` pins them together.
const RANK_BOUNDS: [i32; 9] = [-42000, -6000, -3000, 0, 3000, 9000, 21000, 42000, 43000];

/// Adds the reputation pane's feed and its outbound drain. The bindings live in `benilla-ui`'s
/// `script::reputation`; this supplies their data (and the event) from ECS state.
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

/// One faction's row, or `None` when the faction is not in the player's list at all — no reputation
/// slot, no name, or no race/class mask slot that fits them.
///
/// A faction the pane declines to *draw* is **not** a `None`: that answer rides on
/// [`FactionEntry::visible`], because an unlisted row is still the source of a header's name (see
/// the module doc).
///
/// Split out as a pure function for the same reason [`crate::ui_char::skills_row`] is: the display
/// predicate is the half a test can drive against the real DBC without standing up a VM.
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
        // What GetFactionInfo REPORTS. The toggle itself is stricter (`0x4d5fd0`).
        can_toggle_at_war: flags & flag::PEACE_FORCED == 0 && standing >= -3000,
        inactive: flags & flag::INACTIVE != 0,
    })
}

/// The two cheap inputs [`feed_reputation`]'s gate watches by value — `(race, class, watched
/// faction id)`; the other two are resources, watched with Bevy's own change detection. `None` =
/// not yet read.
type ReputationInputs = Option<(u8, u8, Option<u32>)>;

/// Push the snapshot when one of its inputs moves, and fire `UPDATE_FACTION` when the result
/// actually differs — the whole-snapshot-replace seam [`crate::ui_char::feed_skills`] established,
/// with one addition.
///
/// **The rebuild is gated, unlike the skills feed's.** A snapshot here is 54 rows each carrying a
/// name and a description paragraph, so rebuilding it every frame to discover it is unchanged would
/// allocate ~100 strings a frame forever. Its inputs are exactly four — the wire slots, the
/// catalog, the player's race/class, and the watched index — and all four are cheap to watch, so
/// the expensive build only runs when one of them says something happened. The whole-snapshot
/// equality check stays *behind* that gate: a change to the store that does not change any listed
/// row (a hidden faction's standing ticking up) must not fire the event.
///
/// The event fires on **every** accepted push including the first, which is what the reference's own
/// login seam relies on: `ReputationWatchBar`'s `OnEvent` initializes the watch bar off
/// `UPDATE_FACTION`, and the faction list lands at every login.
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
        // A slot the wire has not covered reads as `(0, 0)` — no flags, no standing — rather than
        // dropping the faction. That is the honest reading (flags of 0 lack VISIBLE, so the row is
        // unlisted, which is exactly "you have not met them"), and it keeps the push COMPLETE:
        // every header's name is carried by its parent's row, so a short or not-yet-arrived
        // standings array must not be able to take a header's label away with it.
        let (flags, wire) = usize::try_from(info.rep_index)
            .ok()
            .and_then(|i| reputations.0.get(i))
            .copied()
            .unwrap_or((0, 0));
        if let Some(row) = reputation_row(faction_id, info, catalog, flags, wire, race, class) {
            entries.push(row);
        }
    }
    // The engine sorts, so the iteration order of the catalog's map must not reach it as noise:
    // sort by the row's own stable identity before the equality check, or a HashMap reshuffle
    // would look like a change and fire UPDATE_FACTION every frame.
    entries.sort_by_key(|e| e.faction_id);

    let fresh = ReputationState { entries, watched };
    if last.as_ref() == Some(&fresh) {
        return;
    }
    script.set_reputation(fresh.clone());
    *last = Some(fresh);
    script.fire_event("UPDATE_FACTION", vec![]);
}

/// Send the pane's queued verbs. None is acked — the engine already flipped its own copy — so this
/// is pure outbound.
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
            // The `None` → `-1` translation the wire needs: slot 0 is the Bloodsail Buccaneers, so
            // a 0 here would watch them rather than clear the bar.
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

    /// [`RANK_BOUNDS`] and [`benilla_formats::reputation_rank`] are the same `PointsInRank` widths
    /// read two ways, so every edge must round-trip: the floor of rank `r` ranks as `r`, and one
    /// below it ranks as `r - 1`. Nothing else keeps the pane's bar from disagreeing with the
    /// nameplate's colour, since only one of them goes through the rank function.
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
        // The widths ARE vmangos's PointsInRank, stated rather than implied.
        let widths: Vec<i32> = RANK_BOUNDS.windows(2).map(|w| w[1] - w[0]).collect();
        assert_eq!(widths, [36000, 3000, 3000, 3000, 6000, 12000, 21000, 1000]);
    }

    /// The display predicate and the base-plus-wire sum, on the real `Faction.dbc`.
    ///
    /// Stormwind (72) is the decisive row, and it exercises the slot pick as well as the sum. Its
    /// four DBC base slots are race-gated — `0x4c` (Dwarf/Night Elf/Gnome) → 3100, `0xb2` (the Horde
    /// races) → −42000, `0x01` (Human) → 4000, then an all-zero slot — so a human matches the THIRD
    /// one and starts at 4000, Friendly. Reading the wire standing alone would show Neutral, and
    /// taking slot 0 because it is first in the row would show 3100: both wrong, and both silently.
    ///
    /// The Horde slot is the same law's other end — an orc reading this faction gets −42000, Hated,
    /// which is how a Horde character sees an Alliance city without anything ever being sent.
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
        // A wire gain rides on top of the base, and carries the rank with it.
        let honored = reputation_row(72, sw, &cat, flag::VISIBLE, 5000, 1, 1).expect("lists");
        assert_eq!(honored.standing, 9000);
        assert_eq!(honored.standing_id, 6, "Honored begins exactly at 9000");
        assert_eq!((honored.bar_min, honored.bar_max), (9000, 21000));

        // The same faction and the same empty wire slot, read by an orc: the Horde-gated slot.
        let orc = reputation_row(72, sw, &cat, flag::VISIBLE, 0, 2, 1).expect("lists");
        assert_eq!(orc.standing, -42000, "the Horde race mask's base");
        assert_eq!(orc.standing_id, 1, "FACTION_STANDING_LABEL1 = Hated");

        // Unmet means NOT VISIBLE, not absent — the row still comes back, carrying its name.
        let unmet = reputation_row(72, sw, &cat, 0, 0, 1, 1).unwrap();
        assert!(!unmet.visible, "unmet");
        assert_eq!(unmet.name, "Stormwind", "and still carries its name");
        // HIDDEN does NOT hide the row: it suppresses the auto-reveal and the rank-change chat
        // notification, and nothing else. Reading it as a list gate — which every emulator's naming
        // invites — would silently drop rows the real client draws.
        let hidden = reputation_row(72, sw, &cat, flag::VISIBLE | flag::HIDDEN, 0, 1, 1).unwrap();
        assert!(hidden.visible, "HIDDEN is not a list gate");
        assert!(!hidden.is_header, "and it is not the header bit either");
    }

    /// **The membership gate, and the slot rule it shares with the base pick.**
    ///
    /// A faction no race/class mask slot fits is not in that character's list at all — the client
    /// calls its add inside the accept block of the very loop that picks the base (`0x4d5555`). On
    /// 1.12's shipped data the gate turns out to exclude **nothing a player can actually roll**, and
    /// this pins that: the complete set of excluded pairs is druids of the six races that cannot be
    /// druids, which is to say no real character at all.
    ///
    /// Cenarion Circle is the row that makes the whole rule visible, and the reason it is asserted
    /// by value: its first slot takes all eight races but a class mask of `0x1df` that deliberately
    /// omits druids, and its second takes Night Elf + Tauren druids at 2000. A rule that stopped at
    /// the FIRST match would hand a Night Elf druid slot 0's zero. Skips without client data.
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

        // The slot rule itself, on that same row: last match wins, and the druid slot is second.
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

    /// **The header factions are flagged, mostly invisible, and must be pushed anyway.**
    ///
    /// This is the bug the `visible` flag exists for, asserted at the site that had it: all five
    /// parents carry the `HEADER` bit and four of the five lack `VISIBLE`, so a feed that filtered
    /// invisible factions out would hand the engine a correct tree with no header names in it — and
    /// the pane would draw the right rows under blank headers. Nothing else catches that: the
    /// grouping is right, the counts are right, and only the label is empty.
    ///
    /// The flags asserted here are the DBC's own defaults for a human warrior, read through the
    /// same race/class slot pick the server seeds a fresh character's state from
    /// (`ReputationMgr::GetDefaultStateFlags`). Skips without client data.
    #[test]
    fn real_header_factions_carry_the_header_bit_and_still_reach_the_engine() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let cat = benilla_formats::load_faction_catalog(&mut chain).expect("factions");
        use benilla_formats::faction_flags as flag;

        // Every faction some reputation faction names as its `team` — the pane's headers.
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
            // The DBC default flag byte a human warrior would be seeded with.
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
        // …and only ONE of the five is VISIBLE as well, which is exactly why visibility cannot be
        // the test for "is this a header" and why an invisible row still has to be pushed.
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

    /// The `SMSG_INITIALIZE_FACTIONS` flag bytes a **live vmangos** sends a fresh level-1 human
    /// warrior, captured 2026-08-13 from this slot's probe character via
    /// `cargo run -p benilla-protocol --example faction_probe`. All 64 standings were 0; every slot
    /// not listed here had a flag byte of 0 too.
    ///
    /// Kept as real bytes rather than a hand-built fixture because this is the one input the whole
    /// pane is a function of, and a plausible-looking invention would agree with whatever the code
    /// did. It also settles, at the wire, the single claim the binary alone could only leave
    /// inferred: that the live server marks exactly the five header factions with `0x08`.
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

    /// **The whole data law, end to end, on real bytes: what a fresh Alliance character's pane
    /// actually says.**
    ///
    /// Live wire flags → `Faction.dbc` → the feed's rows → the engine's tree → the visible row list
    /// `GetNumFactions`/`GetFactionInfo` report. No fixture anywhere in the chain except the capture.
    ///
    /// The expected answer is checkable against the game itself: a brand-new human warrior's
    /// Reputation tab shows the Alliance header and its four city factions, all Neutral-or-better
    /// from their DBC bases alone, and nothing else. Exactly five slots carry `VISIBLE`, and the one
    /// of them that is also a header (Alliance, `0x09`) must come out as the header rather than as a
    /// fifth bar — which is the assertion that would have failed under the emulators' reading of
    /// `0x08`. Skips without client data.
    #[test]
    fn a_fresh_alliance_characters_pane_off_live_wire_bytes() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let cat = benilla_formats::load_faction_catalog(&mut chain).expect("factions");
        let store = live_store();

        // The five header factions, asserted at the wire — the one claim the binary left inferred.
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

        // …and the bars read their DBC bases, with no wire standing anywhere in the capture.
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
