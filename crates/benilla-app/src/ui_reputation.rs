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
//! unit-reaction decode, which caps it at 6, Revered (`0x606439`); the pane shows it uncapped.
//!
//! The chat line "You are now <rank> with <faction>." (`FACTION_STANDING_CHANGED`, `0x62c670`) is
//! owed by three events: a faction newly listed by `SMSG_SET_FACTION_VISIBLE` (`0x4d5733`), a
//! faction listed by the first `SMSG_SET_FACTION_STANDING` that names it (`0x4d5826`), and a
//! standing change that moves the rank (`0x4d5896`) unless the faction is `HIDDEN`. The wire
//! handlers decide the lines ([`visible_line`], [`delta_lines`]) and park them in the chat log in
//! wire order ([`queue_standing_lines`]); the chat feed resolves their strings against the VM,
//! which the wire drain cannot reach.

use bevy::prelude::*;

use benilla_ui::script::{FactionEntry, ReputationSend, ReputationState, UiScript};

use crate::net::{ClientCommand, NetCommands, ObjectStore, Reputations, SelfPlayer};
use crate::target::Factions;
use crate::ui_chat::ChatLog;
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
                feed_reputation
                    .in_set(UiFeed)
                    .in_set(crate::ui_unit::field_events::FieldEventPushes),
                drain_reputation_sends.after(UiInput),
            ),
        );
    }
}

/// One `FACTION_STANDING_CHANGED` line owed: the faction and the rank it names (`0x62c670`'s
/// `ecx` and `edx`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StandingLine {
    pub(crate) faction_id: u32,
    /// The rank, 0 Hated to 7 Exalted; the label is `FACTION_STANDING_LABEL<rank + 1>`.
    pub(crate) rank: u8,
}

/// The faction a wire slot names, with its row, when the player's race and class list it.
fn faction_of_slot(
    catalog: &benilla_formats::FactionCatalog,
    list_id: u32,
    race: u8,
    class: u8,
) -> Option<(u32, &benilla_formats::FactionInfo)> {
    let index = i32::try_from(list_id).ok()?;
    let (id, info) = catalog
        .reputation_factions()
        .find(|(_, f)| f.rep_index == index)?;
    info.slot_for(race, class)?;
    Some((id, info))
}

/// `SMSG_SET_FACTION_VISIBLE` (`0x4d5710`): a slot not yet `VISIBLE` is listed by `0x4d5a70` with
/// the print flag set, which names its rank (`0x4d5bfb`); one already `VISIBLE` returns at
/// `0x4d5731` and prints nothing. `store` is the state before the packet applies. A header slot
/// that a listed child already put in the client's list is a dedup there (`0x4d5a93`); vmangos
/// never sends one (`ReputationMgr.cpp:177`, `SetVisible` refuses flag `0x08`), so it is not
/// modelled.
pub(crate) fn visible_line(
    list_id: u32,
    store: &Reputations,
    catalog: &benilla_formats::FactionCatalog,
    race: u8,
    class: u8,
) -> Option<StandingLine> {
    use benilla_formats::faction_flags as flag;
    let (flags, wire) = store.slots.get(list_id as usize).copied().unwrap_or((0, 0));
    if flags & flag::VISIBLE != 0 {
        return None;
    }
    let (faction_id, info) = faction_of_slot(catalog, list_id, race, class)?;
    Some(StandingLine {
        faction_id,
        rank: benilla_formats::reputation_rank(info.base_for(race, class) + wire),
    })
}

/// `SMSG_SET_FACTION_STANDING` (`0x4d5760`), per entry against the state before it applies. A
/// slot neither `VISIBLE` nor `HIDDEN` is listed first (`0x4d5815`-`0x4d5826`) and that print is
/// the entry's only line (`0x4d588b`); otherwise a changed rank prints unless the slot is `HIDDEN`
/// (`0x4d5880`-`0x4d5896`). Both name the rank after the new standing.
pub(crate) fn delta_lines(
    deltas: &[(u32, i32)],
    store: &Reputations,
    catalog: &benilla_formats::FactionCatalog,
    race: u8,
    class: u8,
) -> Vec<StandingLine> {
    use benilla_formats::faction_flags as flag;
    let mut lines = Vec::new();
    for &(list_id, standing) in deltas {
        let Some((faction_id, info)) = faction_of_slot(catalog, list_id, race, class) else {
            continue;
        };
        let base = info.base_for(race, class);
        let (flags, old) = store.slots.get(list_id as usize).copied().unwrap_or((0, 0));
        let (old_rank, rank) = (
            benilla_formats::reputation_rank(base + old),
            benilla_formats::reputation_rank(base + standing),
        );
        let listed_now = flags & (flag::VISIBLE | flag::HIDDEN) == 0;
        let moved = rank != old_rank && flags & flag::HIDDEN == 0;
        if listed_now || moved {
            lines.push(StandingLine { faction_id, rank });
        }
    }
    lines
}

/// Park the lines in the chat log, after whatever the same packet already queued. The faction's
/// name is the row's (`0x62c670` reads the name column), and `female` picks the label's gender.
pub(crate) fn queue_standing_lines(
    lines: impl IntoIterator<Item = StandingLine>,
    catalog: &benilla_formats::FactionCatalog,
    female: bool,
    chat: &mut ChatLog,
) {
    for StandingLine { faction_id, rank } in lines {
        if let Some(name) = catalog.faction_name(faction_id) {
            chat.push_standing(name, rank, female);
        }
    }
}

/// `0x62c670`'s sentence: `FACTION_STANDING_CHANGED` filled with the rank label and the faction
/// name. The label is `FACTION_STANDING_LABEL<rank + 1>` gendered by `0x612bf0`: a female player
/// tries the `_FEMALE` twin and, when that is absent or empty, the bare key (`0x612c36`). A label
/// absent altogether is an empty string, not a missing line: `0x703bf0` never returns NULL, and
/// `0x62c670` prints with whatever it gave. Only a missing template prints nothing here, where the
/// reference would send an empty string to the chat composer.
pub(crate) fn standing_text(
    get: &dyn Fn(&str) -> Option<String>,
    rank: u8,
    female: bool,
    faction_name: &str,
) -> Option<String> {
    let key = format!("FACTION_STANDING_LABEL{}", rank.min(7) + 1);
    let defined = |k: &str| get(k).filter(|v| !v.is_empty());
    let label = female
        .then(|| defined(&format!("{key}_FEMALE")))
        .flatten()
        .or_else(|| defined(&key))
        .unwrap_or_default();
    let template = defined("FACTION_STANDING_CHANGED")?;
    Some(benilla_ui::strings::fill(
        &template,
        &[
            benilla_ui::strings::Arg::S(&label),
            benilla_ui::strings::Arg::S(faction_name),
        ],
    ))
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
pub(crate) fn feed_reputation(
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
            .and_then(|i| reputations.slots.get(i))
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
    let bare = faction_fires(last.as_ref(), &fresh);
    script.set_reputation(fresh.clone());
    *last = Some(fresh);
    for _ in 0..bare {
        script.fire_event("UPDATE_FACTION", vec![]);
    }
}

/// How many bare `UPDATE_FACTION` signals a transition makes. The standing, flag and rebuild
/// producers (`0x4d569e`, `0x4d56f6`, `0x4d5984`, `0x4d5db0`) signal bare through `0x703e50`; a
/// first push is the initialize message, which signals twice: the rebuild `0x4d5430` calls the
/// sorter `0x4d5c40` (`0x4d55de`), which ends in a bare signal (`0x4d5db0`), and the message ends
/// in another (`0x4d569e`). The watched index is pushed here and signals `("player")` from its
/// own field watch (`0x5ddcce`, [`crate::field_watch`]), never from this diff.
fn faction_fires(prev: Option<&ReputationState>, fresh: &ReputationState) -> usize {
    match prev {
        None => 2,
        Some(p) => usize::from(p.entries != fresh.entries),
    }
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

    /// A live feed over an empty catalog: the first push is the initialize message's two bare
    /// signals, and a watched-index change is pushed with no signal, its watch's to fire.
    #[test]
    fn the_list_signals_bare_and_the_watched_index_is_pushed_silently() {
        use benilla_formats::FactionCatalog;
        const FIELD_WATCHED: u16 = 1261;
        let mut app = App::new();
        app.init_resource::<Reputations>()
            .insert_resource(Factions::from_catalog(FactionCatalog::from_rows(
                Default::default(),
                Default::default(),
            )))
            .add_systems(Update, feed_reputation);
        let script = UiScript::new().unwrap();
        script
            .run(
                r#"
                SEEN = {}
                local f = CreateFrame("Frame")
                f:RegisterEvent("UPDATE_FACTION")
                f:SetScript("OnEvent", function()
                    table.insert(SEEN, event .. " " .. tostring(arg1))
                end)
            "#,
            )
            .unwrap();
        app.insert_non_send_resource(script);
        let me = app
            .world_mut()
            .spawn((
                SelfPlayer,
                ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(
                    FIELD_WATCHED,
                    u32::MAX,
                )])),
            ))
            .id();
        let seen = |app: &mut App| {
            app.update();
            let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
            s.resolve();
            let out = s.eval::<Vec<String>>("return SEEN").unwrap();
            s.run("SEEN = {}").unwrap();
            out
        };
        assert_eq!(
            seen(&mut app),
            vec!["UPDATE_FACTION nil"; 2],
            "initialize: the sorter's signal and the message's"
        );
        assert!(seen(&mut app).is_empty());
        *app.world_mut().get_mut::<ObjectStore>(me).unwrap() = ObjectStore(
            benilla_protocol::ObjectFields::from_pairs(&[(FIELD_WATCHED, 3)]),
        );
        assert!(seen(&mut app).is_empty());
    }

    /// The list's bare signals: two for the initialize message, one per list change, none for the
    /// watched index alone.
    #[test]
    fn the_list_and_the_watched_index_are_two_producers() {
        let entry = |id: u32| FactionEntry {
            faction_id: id,
            rep_list_id: id,
            parent_id: 0,
            name: String::new(),
            description: String::new(),
            standing: 0,
            standing_id: 4,
            bar_min: 0,
            bar_max: 3000,
            visible: true,
            is_header: false,
            at_war: false,
            can_toggle_at_war: true,
            inactive: false,
        };
        let base = ReputationState {
            entries: vec![entry(1)],
            watched: None,
        };
        assert_eq!(faction_fires(None, &base), 2);
        let watched = ReputationState {
            watched: Some(3),
            ..base.clone()
        };
        assert_eq!(faction_fires(Some(&base), &watched), 0);
        let listed = ReputationState {
            entries: vec![entry(1), entry(2)],
            ..base.clone()
        };
        assert_eq!(faction_fires(Some(&base), &listed), 1);
        let both = ReputationState {
            entries: vec![entry(2)],
            watched: Some(1),
        };
        assert_eq!(faction_fires(Some(&base), &both), 1);
    }

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

    /// A human warrior (race 1, class 1) and Stormwind's slot on the real DBC: base 4000, Friendly.
    fn stormwind() -> Option<(benilla_formats::FactionCatalog, u32)> {
        let data = benilla_formats::wow_data_or_skip!(None);
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let cat = benilla_formats::load_faction_catalog(&mut chain).expect("factions");
        let slot = u32::try_from(cat.reputation_faction(72).expect("Stormwind").rep_index).unwrap();
        Some((cat, slot))
    }

    fn store(slot: u32, flags: u8, standing: i32) -> Reputations {
        let mut v = vec![(0u8, 0i32); slot as usize + 1];
        v[slot as usize] = (flags, standing);
        Reputations::from_slots(v)
    }

    /// `0x4d5731` gates the list on `VISIBLE`: an unmet faction prints its rank (base 4000 is
    /// Friendly, rank 4), one already listed prints nothing (the dedup return).
    #[test]
    fn a_newly_listed_faction_names_its_rank_and_a_listed_one_does_not() {
        use benilla_formats::faction_flags as flag;
        let Some((cat, slot)) = stormwind() else {
            return;
        };
        assert_eq!(
            visible_line(slot, &store(slot, 0, 0), &cat, 1, 1),
            Some(StandingLine {
                faction_id: 72,
                rank: 4
            })
        );
        // An empty store is the same first contact: the slot reads (0, 0).
        assert!(visible_line(slot, &Reputations::default(), &cat, 1, 1).is_some());
        assert_eq!(
            visible_line(slot, &store(slot, flag::VISIBLE, 0), &cat, 1, 1),
            None,
            "already listed"
        );
        // The wire standing rides on the base: 5000 more is 9000, Honored.
        assert_eq!(
            visible_line(slot, &store(slot, 0, 5000), &cat, 1, 1).map(|l| l.rank),
            Some(5)
        );
        // A slot the player's race and class do not list, or no faction names: nothing.
        assert_eq!(visible_line(63, &Reputations::default(), &cat, 1, 1), None);
    }

    /// `0x4d5760`: a rank change prints the new rank, an unchanged rank prints nothing, `HIDDEN`
    /// silences a change, and a faction listed by this very entry prints once whatever it moved.
    #[test]
    fn a_standing_delta_prints_a_rank_change_and_a_first_listing_once() {
        use benilla_formats::faction_flags as flag;
        let Some((cat, slot)) = stormwind() else {
            return;
        };
        let lines = |flags: u8, old: i32, new: i32| {
            delta_lines(&[(slot, new)], &store(slot, flags, old), &cat, 1, 1)
        };
        let rank = |r: u8| {
            vec![StandingLine {
                faction_id: 72,
                rank: r,
            }]
        };
        // 4000 → 9000 crosses into Honored.
        assert_eq!(lines(flag::VISIBLE, 0, 5000), rank(5));
        // 4000 → 4100 stays Friendly; no line.
        assert!(lines(flag::VISIBLE, 0, 100).is_empty());
        // Down a rank is a change too.
        assert_eq!(lines(flag::VISIBLE, 0, -1001), rank(3));
        // HIDDEN keeps the gain off the screen (`0x4d5885`).
        assert!(lines(flag::VISIBLE | flag::HIDDEN, 0, 5000).is_empty());
        // Not yet listed: the reveal prints, with no second line for the rank it moved.
        assert_eq!(lines(0, 0, 100), rank(4));
        assert_eq!(lines(0, 0, 5000), rank(5));
        // Several entries print one line each, in order.
        let two = delta_lines(
            &[(slot, 5000), (63, 1)],
            &store(slot, flag::VISIBLE, 0),
            &cat,
            1,
            1,
        );
        assert_eq!(two, rank(5), "the unnamed slot prints nothing");
    }

    /// `0x62c670`: `FACTION_STANDING_CHANGED` filled with the label and the name; a female player
    /// takes the `_FEMALE` label where there is one; a missing label or template prints nothing.
    #[test]
    fn the_sentence_is_the_template_with_the_gendered_label_and_the_name() {
        let strings = [
            ("FACTION_STANDING_CHANGED", "You are now %s with %s."),
            ("FACTION_STANDING_LABEL5", "Friendly"),
            ("FACTION_STANDING_LABEL6", "Honored"),
            ("FACTION_STANDING_LABEL6_FEMALE", "Honoured (f)"),
        ];
        let get = |k: &str| {
            strings
                .iter()
                .find(|(key, _)| *key == k)
                .map(|(_, v)| v.to_string())
        };
        assert_eq!(
            standing_text(&get, 4, false, "Stormwind").as_deref(),
            Some("You are now Friendly with Stormwind.")
        );
        assert_eq!(
            standing_text(&get, 5, false, "Stormwind").as_deref(),
            Some("You are now Honored with Stormwind.")
        );
        assert_eq!(
            standing_text(&get, 5, true, "Stormwind").as_deref(),
            Some("You are now Honoured (f) with Stormwind.")
        );
        assert_eq!(
            standing_text(&get, 4, true, "Stormwind").as_deref(),
            Some("You are now Friendly with Stormwind."),
            "a missing twin falls back to the base label"
        );
        assert_eq!(
            standing_text(&get, 0, false, "Stormwind").as_deref(),
            Some("You are now  with Stormwind."),
            "an absent label is an empty one (`0x703bf0` never returns NULL), not a missing line"
        );
        // A `_FEMALE` twin that is defined but empty falls back to the bare key (`0x612c36`).
        let empty_twin = |k: &str| match k {
            "FACTION_STANDING_LABEL6_FEMALE" => Some(String::new()),
            other => get(other),
        };
        assert_eq!(
            standing_text(&empty_twin, 5, true, "Stormwind").as_deref(),
            Some("You are now Honored with Stormwind.")
        );
        assert_eq!(
            standing_text(
                &|k| get(k).filter(|_| k != "FACTION_STANDING_CHANGED"),
                4,
                false,
                "X"
            ),
            None,
            "no template"
        );
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
