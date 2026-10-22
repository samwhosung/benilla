//! `ChrClasses.dbc`, loaded once for its unrelated readers: the pet book (field 4, pet name token),
//! `UnitHasRelicSlot` (field 16) and `crate::spell::mods` (field 15, spell family). Absent when the
//! load fails; each reader then takes the reference's degraded answer: `"PET"`, no relic slot,
//! family 0.

use std::collections::HashMap;

use bevy::prelude::*;

use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_formats::{ChrClasses, ChrRaces};

/// The parsed table; [`benilla_formats::ChrClasses`] documents the columns.
#[derive(Resource)]
pub(crate) struct ChrClassTable(pub(crate) ChrClasses);

/// `ChrRaces.dbc`'s names and tokens, at the install's locale column; `UnitRace` and the race words
/// the chat and social lists print come from it. Absent when the load fails, and no race has a name.
#[derive(Resource)]
pub(crate) struct ChrRaceTable(pub(crate) ChrRaces);

/// Each race's PvP side, the team digit `0x5efe00` reads off the chain: `ChrRaces.dbc` field 2 to
/// that `FactionTemplate.dbc` row's group mask, `& 4` Horde (0), else `& 2` Alliance (1), else no
/// side (-1). Loaded with the race table ([`benilla_formats::load_race_pvp_teams`]), so a chain
/// that adds races gives them their own side; absent when the load fails, and no race has a side.
#[derive(Resource, Clone, Debug, Default)]
pub(crate) struct RacePvpTeams(pub(crate) HashMap<u8, i8>);

impl RacePvpTeams {
    /// The race's team digit, -1 for a race without a row, the engine's bounds failure.
    pub(crate) fn team(&self, race: u8) -> i8 {
        self.0.get(&race).copied().unwrap_or(-1)
    }

    /// The side's `FactionGroup` token, `"Horde"` or `"Alliance"`, `None` for no side.
    pub(crate) fn faction_group(&self, race: u8) -> Option<&'static str> {
        match self.team(race) {
            0 => Some("Horde"),
            1 => Some("Alliance"),
            _ => None,
        }
    }

    /// The stock 1.12 chain's nine rows: group mask 3 (Alliance) for Human, Dwarf, Night Elf,
    /// Gnome and the unplayable Goblin, mask 5 (Horde) for Orc, Undead, Tauren and Troll.
    #[cfg(test)]
    pub(crate) fn stock() -> Self {
        Self(HashMap::from([
            (1, 1),
            (2, 0),
            (3, 1),
            (4, 1),
            (5, 0),
            (6, 0),
            (7, 1),
            (8, 0),
            (9, 1),
        ]))
    }
}

pub(crate) struct ChrClassesPlugin;

impl Plugin for ChrClassesPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Startup,
            (load_chr_classes, load_chr_races).after(AssetSet::Open),
        );
    }
}

fn load_chr_classes(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_chr_classes(&mut chain)
    };
    match loaded {
        Ok(table) => commands.insert_resource(ChrClassTable(table)),
        Err(e) => warn!(
            "chr_classes: ChrClasses.dbc failed to load — every pet book tab reads the client's \
             own \"PET\" fallback, so a warlock's says Pet rather than Demon, no class reads \
             as having a relic slot, and no talent spell modifier can apply (the gate has no \
             class family to match): {e:#}"
        ),
    }
}

fn load_chr_races(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let (loaded, teams) = {
        let mut chain = assets.chain.lock_recover();
        (
            benilla_formats::load_chr_races(&mut chain),
            benilla_formats::load_race_pvp_teams(&mut chain),
        )
    };
    match loaded {
        Ok(table) => commands.insert_resource(ChrRaceTable(table)),
        Err(e) => warn!(
            "chr_races: ChrRaces.dbc failed to load — no unit has a race name or token, so \
             `UnitRace` answers nil and the social lists print no race: {e:#}"
        ),
    }
    match teams {
        Ok(teams) => commands.insert_resource(RacePvpTeams(teams)),
        Err(e) => warn!(
            "chr_races: the races' PvP sides failed to load (ChrRaces.dbc to FactionTemplate.dbc) \
             — no race has a side, so every rank title and the roster seat's faction group read \
             none: {e:#}"
        ),
    }
}
