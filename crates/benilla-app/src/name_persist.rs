//! **The name cache, kept across sessions** — the load/save half of
//! [`crate::names::NameCache`], and benilla's answer to the reference's `WDB/*.wdb` files.
//!
//! ## Why this exists at all
//!
//! Three of benilla's caches are filled by a server round trip: player names, creature templates
//! and pet names. **The reference persists exactly one of the three** — `creaturecache.wdb` — and
//! that is what this module keeps. `'WNAM'` and `'WPNM'` are constructed with persistence
//! **disabled** (`0x554cd0`'s three `push 0x0`, against `'WNPC'`/`'WIDB'`'s `push 0x1; push 0x1`)
//! and are cleared at world-session start instead; a real 1.12 install's `WDB/` holds
//! `creaturecache.wdb`, `npccache.wdb`, `itemcache.wdb` … and no `namecache.wdb`. 1689 read the
//! reference as "all three" and wrote player and pet names to disk too, which is what answered a
//! wiped server's brand-new character with a deleted one's name; **decision 2223** took them
//! back out and put the wipe in ([`NameCache::clear_world_session`]).
//!
//! What survives is the half that carried the value anyway: a city's worth of creature-template
//! queries on zone-in, answered from disk instead of the wire. The **key** is the whole argument —
//! a template entry means the same creature on every realm forever, while a player guid means one
//! character and a pet number one live spawn.
//!
//! ## The law, from the reference
//!
//! Two contracts of the reference's `DBCache.cpp` machine (`0x554b00`–`0x5738c0`) are the ones a
//! re-implementation must honour, and both are about what the cache does *not* do:
//!
//! - **The header is compared by equality, and carries no checksum, no timestamp and no TTL.** Its
//!   20 bytes are `[FourCC | build 0x16f3 | locale | recordSize | version 1]`. A mismatch discards
//!   the file wholesale. Ours is a header line with the same job ([`NameCache::to_tsv`]).
//! - **Eviction is explicit only** — a high-bit key in a response, or `SMSG_INVALIDATE_PLAYER`
//!   (`0x31C`). Nothing ages out. That is survivable for a record whose key cannot change meaning,
//!   which is precisely why the stores whose keys *can* are not persisted at all.
//!
//! ## Where it lives, and the one place we deviate
//!
//! The reference writes `WDB/` **inside the install**, which is precisely where benilla may not
//! write (`docs/METHOD.md`'s read-only rule). Ours goes to `benilla-config/cache/<realm>.tsv` through
//! [`crate::local_state`], like every other thing we persist — and it is **realm-scoped**, which
//! the reference's is not. That is a fix rather than a preference: every key is realm-local (a
//! player guid, a creature entry, a pet number), so one shared file would serve another realm's
//! names to this one.

use std::path::PathBuf;

use bevy::prelude::*;

use crate::char_select::Roster;
use crate::names::NameCache;

/// How long a run of landed answers is allowed to accumulate before it is written. The cache is
/// worth keeping but never worth a stall: names arrive in bursts (a city's worth of creature
/// queries on zone-in), and writing the whole file per answer would turn a burst into hundreds of
/// rewrites. A crash inside the window costs re-asking, which is exactly what the cache is for.
const SAVE_DEBOUNCE: f32 = 10.0;

pub(crate) struct NamePersistPlugin;

impl Plugin for NamePersistPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NameCacheFile>()
            .add_systems(Update, (load_name_cache, save_name_cache).chain());
        // The exit edge: the last burst of answers is worth the one write, and it
        // is the only chance to take it — the debounce above will not fire again.
        crate::shutdown::on_app_exit(app, save_on_exit.into_configs());
    }
}

/// Where this realm's cache lives and what we last wrote there.
#[derive(Resource, Default)]
pub(crate) struct NameCacheFile {
    /// The realm the loaded file belongs to; `None` before the first login. A *change* is what
    /// triggers a load — logging into a second realm must not keep the first's names.
    realm: Option<String>,
    path: Option<PathBuf>,
    /// [`NameCache::generation`] as of the last successful write. The cache's own landed-answer
    /// counter is the dirty bit — it ticks on an arrival and never on an ask, which is exactly the
    /// edge worth writing on.
    saved_generation: u64,
    since_save: f32,
}

/// Load the realm's cache the first frame its identity is known, and on any realm change.
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
        return; // a hermetic capture, or no state folder — session-only, exactly as before
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return; // no cache yet: the ordinary first-run path, not an error
    };
    match NameCache::from_tsv(&text, &realm) {
        Some(loaded) => {
            let n = loaded.len();
            // **The file's own records go in; nothing else is touched**. This used
            // to be `*names = loaded`, on the premise stated here that "world entry clears the
            // guid-keyed stores a moment later regardless" — and the ordering is the other way
            // round. This loader fires the first frame `identity` answers, which is the frame the
            // *pick* is in flight; `connected` does its world-session clear and then seeds our own
            // name in the same neighbourhood. Whichever ran second won, and when it was this one
            // the player's own name went with it — `UnitName("player")` nil for a wire round-trip,
            // once per process, on the first login of a fresh start. See
            // [`NameCache::install_persisted`].
            names.install_persisted(loaded);
            file.saved_generation = names.generation();
            debug!(
                "names: loaded {n} cached records for {realm} from {}",
                path.display()
            );
        }
        // The header did not match — a different build, locale or format. Discarding the whole
        // file is the reference's own rule, and the file is left on disk to be overwritten by the
        // first save rather than deleted out from under a player who may be switching back.
        None => debug!(
            "names: discarding {} — header is not this build/locale/format",
            path.display()
        ),
    }
}

/// Write the cache when answers have landed since the last write and the debounce has elapsed.
fn save_name_cache(time: Res<Time>, mut file: ResMut<NameCacheFile>, names: Res<NameCache>) {
    file.since_save += time.delta_secs();
    if file.since_save < SAVE_DEBOUNCE {
        return;
    }
    file.since_save = 0.0;
    write_now(&mut file, &names);
}

/// The exit-edge write — unconditional on the debounce, because there is no next
/// frame to defer to.
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
        // A cache we could not write is a cache we re-fill next session — worth a line, never
        // worth failing a session over.
        Err(e) => warn!("names: cannot write {}: {e}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_state::test_env::{EnvGuard, ENV_LOCK};

    /// A roster with a pick in flight and no realm — [`crate::ui_macro::identity`] then answers
    /// `("Realm", <name>)`, which is the identity the loader keys the file by.
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

    /// **The director's report, at the seam that produced it**.
    ///
    /// The realm cache is read off disk the first frame the pick's identity is known, and the
    /// login's seed of our OWN name lands in the same neighbourhood — so the two race, and the
    /// loader used to win by overwriting the whole cache (`*names = loaded`). What that cost was
    /// `UnitName("player")` answering **nil** for one wire round-trip on the first login of a
    /// fresh start, because the feed's player snapshot carries the cache's answer and a `None`
    /// there replaces the roster seat the UI loaded under. KLHThreatMeter reads it on its first
    /// `OnUpdate` and indexes a table with it: `KTM_Tables.lua:198: table index is nil`.
    ///
    /// The load runs here *after* the seed on purpose: that is the losing order, and it must now
    /// be survivable rather than merely unlikely.
    #[test]
    fn the_disk_load_installs_templates_without_touching_the_live_session() {
        use benilla_protocol::guid;
        /// `counter | (entry << 24) | (high << 48)` — the server's own composition.
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

        // A realm file with one creature template in it — written the way the saver writes it, so
        // the header the loader gates on is this build's by construction.
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

        // The live session: the login has already seeded our own name (and a pet's), exactly as
        // `net::session::connected` does a moment after the pick goes out.
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
