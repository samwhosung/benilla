//! The realm a session connects to, as every glue screen needs it: the client's realm tables
//! ([`RealmTables`]), the `realmName` CVar, a realm record's down flag, and the run's own answer to
//! a realm list nobody picks from ([`RunModeRealm`]).
//!
//! Not [`crate::realmlist`], which is the logon server's address (the `realmList` CVar).

use bevy::prelude::*;

use benilla_formats::{RealmCategory, RealmConfigs};
use benilla_protocol::RealmInfo;

/// The persisted 1.12 CVar naming the last realm connected to (`0x83f2d0`); the SavedVariables
/// path (`0x8539a8`) is built from it.
pub(crate) const CVAR_REALM_NAME: &str = "realmName";

/// `GetRealmInfo`'s `realmDown`: flag bit 0x02 of the record's flags byte (`+0x08`, `0x46ef83`),
/// greyed and not enterable. The reference's own pick, `SetPreferredInfo`'s `0x46ea20`, skips only
/// bit 0x01 (`0x46ea7c`), so it can suggest a realm that is down; the run's own pick skipping one
/// ([`RunModeRealm`]) is benilla's policy, not the reference's.
pub(crate) fn is_down(realm: &RealmInfo) -> bool {
    realm.flags & 0x02 != 0
}

/// The client's realm tables, read once at startup, which the realm list's tabs and type column
/// and the world entry's PvP latch read.
#[derive(Resource, Default)]
pub(crate) struct RealmTables {
    /// The client Region's `Cfg_Categories.dbc` rows, the category set `0x46e300` builds once per
    /// process; `None` until read, and every realm lists untabbed.
    pub(crate) categories: Option<Vec<RealmCategory>>,
    /// `Cfg_Configs.dbc`, the realm types; empty until read, and every type reads Normal.
    pub(crate) types: RealmConfigs,
}

impl RealmTables {
    /// The categories and the realm types as the stock glue's realm verbs read them; no
    /// categories until the DBC is read.
    pub(crate) fn glue_setup(
        &self,
    ) -> (
        Vec<benilla_ui::script::GlueRealmCategory>,
        Vec<benilla_ui::script::GlueRealmType>,
    ) {
        let categories = self
            .categories
            .iter()
            .flatten()
            .map(|c| benilla_ui::script::GlueRealmCategory {
                id: c.id,
                name: c.name.clone(),
            })
            .collect();
        let types = self
            .types
            .rows()
            .iter()
            .map(|t| benilla_ui::script::GlueRealmType {
                realm_type: t.realm_type,
                pvp: t.pvp,
                rp: t.rp,
            })
            .collect();
        (categories, types)
    }

    /// A realm type's `PlayerKillingAllowed`, `None` for a type with no row.
    pub(crate) fn row_pvp(&self, realm_type: u32) -> Option<bool> {
        self.types.get(realm_type).map(|c| c.pvp)
    }
}

#[cfg(test)]
impl RealmTables {
    /// A US client's tables, read without an install: Region 1's two categories, in
    /// `Cfg_Categories.dbc` file order, and the shipped `Cfg_Configs.dbc` types 0, 1, 6 and 8.
    pub(crate) fn us_client() -> RealmTables {
        let cat = |id, name: &str| RealmCategory {
            id,
            name: name.into(),
        };
        let ty = |realm_type, pvp, rp| benilla_formats::RealmConfig {
            realm_type,
            pvp,
            rp,
        };
        RealmTables {
            categories: Some(vec![cat(1, "United States"), cat(5, "Oceanic")]),
            types: RealmConfigs::from_rows(vec![
                ty(0, false, false),
                ty(1, true, false),
                ty(6, false, true),
                ty(8, true, true),
            ]),
        }
    }
}

/// Read the client Region's category set once, after the archive chain opens.
fn load_categories(
    mut tables: ResMut<RealmTables>,
    assets: Option<Res<benilla_assets::WorldAssets>>,
) {
    use benilla_assets::LockRecover;
    let Some(assets) = assets else {
        return;
    };
    let mut chain = assets.chain.lock_recover();
    let region = benilla_formats::client_region(&chain);
    match benilla_formats::load_realm_categories(&mut chain, region) {
        Ok(categories) => {
            info!(
                "realm list: Region {region}, categories {:?}",
                categories
                    .iter()
                    .map(|c| (c.id, c.name.as_str()))
                    .collect::<Vec<_>>()
            );
            if categories.is_empty() {
                warn!(
                    "realm list: Region {region} names no Cfg_Categories.dbc row, so every realm \
                     lists on one untabbed page (the reference's Region is Wow.ini's [WoW Config] \
                     Region)"
                );
            }
            tables.categories = Some(categories);
        }
        Err(e) => warn!(
            "realm list: Cfg_Categories.dbc failed to load, so every realm lists on one untabbed \
             page: {e:#}"
        ),
    }
}

/// Read `Cfg_Configs.dbc` once, after the archive chain opens.
fn load_realm_types(
    mut tables: ResMut<RealmTables>,
    assets: Option<Res<benilla_assets::WorldAssets>>,
) {
    use benilla_assets::LockRecover;
    let Some(assets) = assets else {
        return;
    };
    let mut chain = assets.chain.lock_recover();
    match benilla_formats::load_realm_configs(&mut chain) {
        Ok(types) => {
            info!(
                "realm list: {} Cfg_Configs.dbc realm types",
                types.rows().len()
            );
            tables.types = types;
        }
        Err(e) => warn!(
            "realm list: Cfg_Configs.dbc failed to load, so every realm reads Normal and no realm \
             is PvP: {e:#}"
        ),
    }
}

/// The run's own answer to a realm list nobody picks from, benilla's and not the reference's,
/// which shows a player with no realm to dial the list or the realm wizard: `WOW_REALM` names a
/// realm, and an unattended run ([`crate::run_mode::unattended`]), with nobody to click Okay, takes
/// one. A player's run has neither, so its list stands as the reference's does. Read once, when
/// the app builds.
#[derive(Resource, Clone, Default, Debug)]
pub(crate) struct RunModeRealm {
    /// `WOW_REALM`, matched case-folded; an empty value is none.
    named: Option<String>,
    unattended: bool,
}

impl RunModeRealm {
    /// The choice as this process's environment makes it.
    pub(crate) fn from_env() -> Self {
        Self {
            named: std::env::var("WOW_REALM").ok().filter(|s| !s.is_empty()),
            unattended: crate::run_mode::unattended(),
        }
    }

    /// A choice as a test makes it.
    #[cfg(test)]
    pub(crate) fn new(named: Option<&str>, unattended: bool) -> Self {
        Self {
            named: named.map(str::to_string),
            unattended,
        }
    }

    /// `WOW_REALM`.
    pub(crate) fn named(&self) -> Option<&str> {
        self.named.as_deref()
    }

    /// The realm an unattended run takes off `realms`, a list in the server's order: the first up
    /// of those holding the most of the account's characters, so a probe returns to the realm it
    /// plays on. `None` for an attended run or a list with no realm up.
    pub(crate) fn unattended_pick<'a>(&self, realms: &'a [RealmInfo]) -> Option<&'a RealmInfo> {
        if !self.unattended {
            return None;
        }
        let mut up: Vec<&RealmInfo> = realms.iter().filter(|r| !is_down(r)).collect();
        // Stable: ties keep the server's order.
        up.sort_by_key(|r| std::cmp::Reverse(r.characters));
        up.first().copied()
    }
}

/// The first of `names` that names a realm on `realms` that is up, case folded, since vmangos does
/// not normalise realm names.
pub(crate) fn first_named_up<'a, 'n>(
    realms: &'a [RealmInfo],
    names: impl IntoIterator<Item = &'n str>,
) -> Option<&'a RealmInfo> {
    names.into_iter().find_map(|want| {
        realms
            .iter()
            .find(|r| r.name.eq_ignore_ascii_case(want) && !is_down(r))
    })
}

pub(super) fn build(app: &mut App) {
    app.init_resource::<RealmTables>()
        .insert_resource(RunModeRealm::from_env())
        .add_systems(
            Startup,
            (load_categories, load_realm_types).after(benilla_assets::AssetSet::Open),
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn realm(name: &str, flags: u8, characters: u8) -> RealmInfo {
        RealmInfo {
            name: name.into(),
            address: "127.0.0.1:8085".into(),
            population: 1.0,
            characters,
            realm_type: 0,
            flags,
            category: 1,
            id: 0,
        }
    }

    /// The US client reads its two categories from the install's own `Cfg_Categories.dbc`.
    #[test]
    fn the_us_categories_come_from_the_install() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let loaded = benilla_formats::load_realm_categories(&mut chain, 1).expect("DBC");
        assert_eq!(Some(loaded), RealmTables::us_client().categories);
    }

    /// A name matches case folded and only a realm that is up; the names are tried in order.
    #[test]
    fn a_named_realm_matches_case_folded_and_up() {
        let list = [
            realm("Down", 0x02, 0),
            realm("Alpha", 0, 0),
            realm("Mu", 0, 0),
        ];
        let named = |names: &[&str]| first_named_up(&list, names.iter().copied()).map(|r| &*r.name);
        assert_eq!(named(&["mu"]), Some("Mu"));
        assert_eq!(
            named(&["down"]),
            None,
            "a realm that is down is never named"
        );
        assert_eq!(named(&["Gone", "down", "ALPHA", "Mu"]), Some("Alpha"));
    }

    /// The unattended pick: nothing for an attended run; else the first realm up holding the most
    /// characters, ties in the server's order.
    #[test]
    fn the_unattended_pick_prefers_the_realm_the_account_plays_on() {
        let list = [
            realm("Down", 0x02, 9),
            realm("Alpha", 0, 0),
            realm("Mu", 0, 2),
            realm("Zeta", 0, 2),
        ];
        let pick = |run: RunModeRealm| run.unattended_pick(&list).map(|r| r.name.clone());
        assert_eq!(pick(RunModeRealm::new(None, false)), None);
        assert_eq!(pick(RunModeRealm::new(None, true)).as_deref(), Some("Mu"));
        assert_eq!(
            RunModeRealm::new(None, true)
                .unattended_pick(&list[..2])
                .map(|r| &*r.name),
            Some("Alpha"),
            "no characters anywhere: the first realm up"
        );
        assert!(
            RunModeRealm::new(None, true)
                .unattended_pick(&list[..1])
                .is_none(),
            "no realm up: no pick"
        );
    }
}
