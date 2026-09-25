//! The creature-template half of [`crate::names::NameCache`], kept across sessions: benilla's
//! `creaturecache.wdb`.
//!
//! The reference persists only `'WNPC'`; `'WNAM'` and `'WPNM'` (player and pet names) are built
//! with persistence off (`0x554cd0`) and cleared at world-session start
//! ([`NameCache::clear_world_session`]). A template entry keeps its meaning, while a player guid
//! or pet number can come to name another character or spawn.
//!
//! From `DBCache.cpp` (`0x554b00`-`0x5738c0`): the 20-byte header
//! `[FourCC | build 0x16f3 | locale | recordSize | version 1]` is compared by equality, with no
//! checksum or TTL, and a mismatch discards the file ([`NameCache::to_tsv`] writes ours).
//! Eviction is explicit only: a high-bit key in a response, or `SMSG_INVALIDATE_PLAYER` (`0x31C`).
//!
//! Deviation: the file is `benilla-config/cache/<realm>.tsv` through [`crate::local_state`], not
//! `WDB/` in the install, because the install is read-only; and it is per realm, because another
//! server may hold a different template under the same entry.

use std::path::PathBuf;

use bevy::prelude::*;

use crate::char_select::Roster;
use crate::names::NameCache;

/// Seconds between writes: answers arrive in bursts on zone-in, and a crash costs only re-asking.
const SAVE_DEBOUNCE: f32 = 10.0;

pub(crate) struct NamePersistPlugin;

impl Plugin for NamePersistPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NameCacheFile>()
            .add_systems(Update, (load_name_cache, save_name_cache).chain());
        // The exit write catches the last burst, which the debounce would not.
        crate::shutdown::on_app_exit(app, save_on_exit.into_configs());
    }
}

/// Where this realm's cache lives and what we last wrote there.
#[derive(Resource, Default)]
pub(crate) struct NameCacheFile {
    /// The realm the loaded file belongs to; a change triggers a load.
    realm: Option<String>,
    path: Option<PathBuf>,
    /// [`NameCache::generation`] at the last write: the dirty bit, which ticks on an answer.
    saved_generation: u64,
    since_save: f32,
}

/// Loads the realm's cache the first frame its identity is known, and on a realm change.
fn load_name_cache(
    roster: Res<Roster>,
    mut file: ResMut<NameCacheFile>,
    mut names: ResMut<NameCache>,
) {
    let Some((realm, _)) = crate::ui_macro::identity(&roster) else {
        return;
    };
    if file.realm.as_deref() == Some(realm.as_str()) {
        return;
    }
    let path = crate::local_state::name_cache_path(&realm);
    file.realm = Some(realm.clone());
    file.path = path.clone();
    file.saved_generation = names.generation();
    let Some(path) = path else {
        return; // a hermetic capture, or no state folder: session-only
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return; // no cache yet
    };
    match NameCache::from_tsv(&text, &realm) {
        Some(loaded) => {
            let n = loaded.len();
            // Install only the file's records: this runs as the pick is in flight, beside the
            // login's clear and seed of our own name, and must not replace them.
            names.install_persisted(loaded);
            file.saved_generation = names.generation();
            debug!(
                "names: loaded {n} cached records for {realm} from {}",
                path.display()
            );
        }
        // A header mismatch discards the file, as the reference does; the first save overwrites it.
        None => debug!(
            "names: discarding {} — header is not this build/locale/format",
            path.display()
        ),
    }
}

/// Writes the cache when answers have landed and the debounce has elapsed.
fn save_name_cache(time: Res<Time>, mut file: ResMut<NameCacheFile>, names: Res<NameCache>) {
    file.since_save += time.delta_secs();
    if file.since_save < SAVE_DEBOUNCE {
        return;
    }
    file.since_save = 0.0;
    write_now(&mut file, &names);
}

/// The exit write, regardless of the debounce.
fn save_on_exit(mut file: ResMut<NameCacheFile>, names: Res<NameCache>) {
    write_now(&mut file, &names);
}

fn write_now(file: &mut NameCacheFile, names: &NameCache) {
    if names.generation() == file.saved_generation {
        return; // nothing landed since the last write
    }
    let (Some(path), Some(realm)) = (file.path.clone(), file.realm.clone()) else {
        return;
    };
    if let Some(dir) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(dir) {
            warn!("names: cannot create {}: {e}", dir.display());
            return;
        }
    }
    match crate::local_state::write_atomic(&path, &names.to_tsv(&realm)) {
        Ok(()) => {
            file.saved_generation = names.generation();
            debug!("names: wrote {} records to {}", names.len(), path.display());
        }
        Err(e) => warn!("names: cannot write {}: {e}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_state::test_env::{EnvGuard, ENV_LOCK};

    /// A roster with a pick in flight and no realm: [`crate::ui_macro::identity`] answers
    /// `("Realm", <name>)`.
    fn roster_with_pick(name: &str, guid: u64) -> Roster {
        Roster::with_pending_pick(
            vec![benilla_protocol::Character {
                guid,
                name: name.into(),
                race: 1,
                class: 1,
                gender: 0,
                skin: 0,
                face: 0,
                hair_style: 0,
                hair_color: 0,
                facial_hair: 0,
                level: 1,
                zone: 0,
                map: 0,
                position: benilla_protocol::wire::Vector3d {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                },
                flags: 0,
                equipment: [benilla_protocol::CharEnumItem::default(); 19],
                pet_display_id: 0,
                pet_level: 0,
                pet_family: 0,
            }],
            guid,
        )
    }

    /// The load runs after the login's seed of our own name, the order that would lose it, which
    /// left `UnitName("player")` nil on the first login.
    #[test]
    fn the_disk_load_installs_templates_without_touching_the_live_session() {
        use benilla_protocol::guid;
        /// `counter | (entry << 24) | (high << 48)`, the server's composition.
        fn compose(high: u16, entry: u32, counter: u32) -> u64 {
            u64::from(counter) | (u64::from(entry) << 24) | (u64::from(high) << 48)
        }
        const CREATURE_ENTRY: u32 = 1234;
        const PET_NUMBER: u32 = 7;
        let me = compose(guid::HIGH_PLAYER, 0, 0x2A);
        let pet = compose(guid::HIGH_PET, PET_NUMBER, 9);
        let guard = compose(guid::HIGH_UNIT, CREATURE_ENTRY, 1);

        let _l = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = std::env::temp_dir().join(format!("benilla-namecache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let _capture = EnvGuard::unset("WOW_CAPTURE");
        let _home = EnvGuard::set("BENILLA_HOME", tmp.to_str().expect("utf-8 temp path"));

        // A realm file with one creature template, written by the saver so the header matches.
        let mut on_disk = NameCache::default();
        on_disk.insert_creature(
            CREATURE_ENTRY,
            Some(crate::names::CreatureRecord {
                name: "Stormwind Guard".into(),
                subname: None,
                creature_type: 7,
                pet_family: 0,
                rank: 0,
                type_flags: 0,
                civilian: false,
                racial_leader: false,
                display_id: 0,
            }),
        );
        let path = crate::local_state::name_cache_path("Realm").expect("a state folder");
        std::fs::create_dir_all(path.parent().expect("cache dir")).expect("cache dir");
        std::fs::write(&path, on_disk.to_tsv("Realm")).expect("write the realm cache");

        // The login has already seeded our own name and a pet's, as `net::session::connected` does.
        let mut names = NameCache::default();
        names.insert_player(me, "Nelprifour".into(), None);
        names.insert_pet(PET_NUMBER, "Fluffy".into());
        let before = names.generation();

        let mut app = App::new();
        app.insert_resource(roster_with_pick("Nelprifour", me))
            .insert_resource(names)
            .init_resource::<NameCacheFile>()
            .add_systems(Update, load_name_cache);
        app.update();

        let names = app.world().resource::<NameCache>();
        assert_eq!(
            names.peek(me),
            Some("Nelprifour"),
            "the login's seed of our own name must survive the disk load — losing it is the bug"
        );
        assert_eq!(
            names.peek(pet),
            Some("Fluffy"),
            "…and so must a pet name: the file carries neither, so neither is its to replace"
        );
        assert_eq!(
            names.peek(guard),
            Some("Stormwind Guard"),
            "the file's own records are what the load is for"
        );
        assert!(
            names.generation() > before,
            "a landed record moves the counter the gated feeds watch (1439)"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
