//! Unit-name resolution. Descriptors carry no names: a player's answers `CMSG_NAME_QUERY` (by
//! guid), a creature's `CMSG_CREATURE_QUERY` (by template entry), a pet's or charm's
//! `CMSG_PET_NAME_QUERY` (by pet number), and which one applies is read off the descriptor as the
//! reference's `GetUnitName` (`0x609210`) reads it. Each key is asked once while in flight, and a
//! negative answer is cached so a bad id never loops; for a player that is an empty name, which the
//! reference evicts instead (`0x55f6f0`), so its next read asks again.
//!
//! Creature templates persist, as the reference's `creaturecache.wdb` does; player and pet names
//! are never written to disk and are cleared at world-session start, as the reference does.

use std::collections::HashMap;

use bevy::prelude::*;

use benilla_protocol::guid;

use crate::net::{ClientCommand, NetCommands, ObjectStore};
use crate::query_cache::QueryCache;

/// The creature template's `type_flags` dword from `SMSG_CREATURE_QUERY_RESPONSE`, cached by the
/// reference at `[CGUnit+0xb30] + 0x14`. Adjacent bits mean unrelated things, and the reference
/// gives each its own getter, so every consumer names the bit it reads:
///
/// | bit | reference getter | meaning |
/// |---|---|---|
/// | `0x1` | `0x529d15` | TAMEABLE |
/// | `0x2` | `0x605f70` | VISIBLE_TO_GHOSTS |
/// | `0x4` | `0x612530` | BOSS_MOB |
/// | `0x8` | `0x6125f0` | [`DO_NOT_PLAY_WOUND_ANIM`] |
/// | `0x10` | `0x612610` (inverted) | [`NO_FACTION_TOOLTIP`] |
/// | `0x20` | `0x623b70` | [`MORE_AUDIBLE`] |
/// | `0x40` | `0x60d840` | SPELL_ATTACKABLE / no harmful vertex colouring |
/// | `0x80` | `0x613230` = `CanInteractWhileDead` | INTERACT_WHILE_DEAD |
///
/// The names of `0x1`-`0x40` follow vmangos `CreatureDefines.h:146-152`.
pub(crate) mod type_flags {
    /// `0x8`: the reference skips the victim wound flinch for it (`0x60ea9f` in `0x60ea70`); the
    /// blood spurt and the floating combat text still play.
    pub(crate) const DO_NOT_PLAY_WOUND_ANIM: u32 = 0x8;

    /// `0x10`: the unit tooltip drops its faction line (`0x612610`, which returns it inverted).
    pub(crate) const NO_FACTION_TOOLTIP: u32 = 0x10;

    /// `0x20`: the pass-2 election keeps an off-screen creature ticking so its combat stays
    /// audible (`0x607da0`'s `0x623b70` arm).
    pub(crate) const MORE_AUDIBLE: u32 = 0x20;
}

/// The name cache: players by guid, creatures by template entry, pets by pet number.
#[derive(Resource, Default)]
pub(crate) struct NameCache {
    /// A `None` answer is an empty wire name: the server does not know the guid.
    players: QueryCache<u64, String>,
    /// `(race, class, gender)` from `SMSG_NAME_QUERY_RESPONSE`, the `$`-macro expander's fallback
    /// for an unstreamed subject (`0x506f70`). Present only when the server answered: a name
    /// learned otherwise (our own, seeded at login) has no entry, never a zero triple.
    player_traits: HashMap<u64, (u8, u8, u8)>,
    /// A `None` answer is an entry the server flagged unknown.
    creatures: QueryCache<u32, CreatureRecord>,
    /// No negative entry: the server answers for a live pet or says nothing.
    pets: QueryCache<u32, String>,
    /// Bumped by every landed answer and the pet-rename eviction, never by an ask: the gated
    /// feeds re-run when it moves.
    generation: u64,
}

/// One cached creature template head.
#[derive(Clone, Debug)]
pub(crate) struct CreatureRecord {
    pub(crate) name: String,
    pub(crate) subname: Option<String>,
    pub(crate) creature_type: u32,
    /// `CreatureFamily.dbc` id, 0 for anything not a tameable beast or warlock minion (the table
    /// has no row 0).
    pub(crate) pet_family: u32,
    /// Elite rank 0..4 as the template declares it; read it through [`gated_rank`].
    pub(crate) rank: u32,
    /// Read bit by bit through [`type_flags`].
    pub(crate) type_flags: u32,
    pub(crate) civilian: bool,
    /// The tooltip's LEADER line (`0x6125c0`).
    pub(crate) racial_leader: bool,
    /// `CreatureDisplayInfo.dbc` id, the first of the template's display ids, 0 when it has none.
    /// The only model for a unit with no world object, such as a stabled pet; an on-screen unit's
    /// own `UNIT_FIELD_DISPLAYID` wins.
    pub(crate) display_id: u32,
}

/// The 1.12 client's creature-rank getter (`0x605620`): 0 before the record is queried and 0 for
/// any unit with a `UNIT_FIELD_PETNUMBER` (a charmed or enslaved unit), else the template rank.
/// `UnitClassification`, the tooltip's ELITE/BOSS word and `UnitLevel`'s world-boss -1 all read
/// through it, so every rank read here goes through this function. No store reads as not-a-pet.
pub(crate) fn gated_rank(rec: Option<&CreatureRecord>, store: Option<&ObjectStore>) -> u32 {
    match rec {
        Some(rec) if !store.is_some_and(|s| s.0.unit_is_pet_or_charm()) => rec.rank,
        _ => 0,
    }
}

/// Which cache names a unit and under which key: the branch of the reference's `GetUnitName`
/// (`0x609210`), which the combat log's `GetObjectName` (`0x6264e0`) also calls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NameKey {
    /// The `OBJECT_FIELD_TYPE` player bit (`0x609232`): the name cache by guid.
    Player(u64),
    /// `UNIT_FIELD_PETNUMBER` non-zero (`0x609295`): the pet-name cache under that number.
    Pet(u32),
    /// `UNIT_FIELD_PETNUMBER` zero (`0x60929d`): the creature record under the descriptor's
    /// `OBJECT_FIELD_ENTRY` (`0x60b160`).
    Creature(u32),
}

impl NameKey {
    /// With a descriptor this is `0x609210` exactly: the pet number and entry are the unit's
    /// fields, not the guid's bits (a vmangos companion pet has a `HIGHGUID_PET` guid but
    /// `PETNUMBER` 0; a charmed creature has a `HIGHGUID_UNIT` guid and a pet number). Without
    /// one, a `HIGHGUID_PET` guid's entry slot is taken as the pet number, right only for a
    /// permanent pet.
    fn of(guid_val: u64, unit: Option<&ObjectStore>) -> Option<Self> {
        if guid::is_player(guid_val) {
            return Some(Self::Player(guid_val));
        }
        if !guid::is_creature_or_pet(guid_val) {
            return None;
        }
        match unit {
            Some(unit) => match unit.0.unit_pet_number() {
                0 => unit
                    .0
                    .object_entry()
                    .filter(|&e| e != 0)
                    .or_else(|| guid::entry(guid_val))
                    .map(Self::Creature),
                n => Some(Self::Pet(n)),
            },
            None => guid::pet_number(guid_val)
                .map(Self::Pet)
                .or_else(|| guid::entry(guid_val).map(Self::Creature)),
        }
    }
}

impl NameCache {
    /// The name for a guid with no descriptor in hand; on a miss, asks once and returns `None`.
    /// A caller holding the unit's [`ObjectStore`] uses [`Self::resolve_unit`]: the guid alone
    /// cannot name a companion pet or a guardian. A game object is `None` without a query.
    pub(crate) fn resolve(&self, guid_val: u64, commands: &NetCommands) -> Option<&str> {
        self.resolve_unit(guid_val, None, commands)
    }

    /// The name for a unit keyed off its descriptor, as `0x609210` keys it; asks once on a miss.
    pub(crate) fn resolve_unit(
        &self,
        guid_val: u64,
        unit: Option<&ObjectStore>,
        commands: &NetCommands,
    ) -> Option<&str> {
        match NameKey::of(guid_val, unit)? {
            NameKey::Player(guid_val) => self
                .players
                .get_or_ask(guid_val, || {
                    debug!("names: asking player name (guid {guid_val})");
                    let _ = commands.0.send(ClientCommand::NameQuery { guid: guid_val });
                })
                .map(String::as_str),
            NameKey::Pet(pet_number) => self.resolve_pet(pet_number, guid_val, commands),
            NameKey::Creature(entry) => self.resolve_creature(entry, guid_val, commands),
        }
    }

    /// There is no negative answer: vmangos sends nothing when the guid is not a live pet with
    /// that number (`PetHandler.cpp:190-192`).
    fn resolve_pet(&self, pet_number: u32, guid: u64, commands: &NetCommands) -> Option<&str> {
        self.pets
            .get_or_ask(pet_number, || {
                debug!("names: asking pet name (pet {pet_number})");
                let _ = commands
                    .0
                    .send(ClientCommand::PetNameQuery { pet_number, guid });
            })
            .map(String::as_str)
    }

    /// The name for a creature template `entry`; `guid` is 0 when no spawn is known, since the
    /// server answers by entry alone.
    pub(crate) fn resolve_creature(
        &self,
        entry: u32,
        guid: u64,
        commands: &NetCommands,
    ) -> Option<&str> {
        self.creatures
            .get_or_ask(entry, || {
                debug!("names: asking creature name (entry {entry})");
                let _ = commands
                    .0
                    .send(ClientCommand::CreatureQuery { entry, guid });
            })
            .map(|r| r.name.as_str())
    }

    /// The cached name for `guid`, with no query on a miss.
    pub(crate) fn peek(&self, guid_val: u64) -> Option<&str> {
        self.peek_unit(guid_val, None)
    }

    /// The cached name for a unit keyed off its descriptor, with no query on a miss.
    pub(crate) fn peek_unit(&self, guid_val: u64, unit: Option<&ObjectStore>) -> Option<&str> {
        match NameKey::of(guid_val, unit)? {
            NameKey::Player(guid_val) => self.players.get(guid_val).map(String::as_str),
            NameKey::Pet(pet_number) => self.pets.get(pet_number).map(String::as_str),
            NameKey::Creature(entry) => self.creatures.get(entry).map(|r| r.name.as_str()),
        }
    }

    /// Record a player-name answer; an empty name is a negative answer. `traits` is `None` for a
    /// name learned without `SMSG_NAME_QUERY_RESPONSE`, and then clears any traits filed for the
    /// guid, since they belonged to the name being replaced.
    pub(crate) fn insert_player(&mut self, guid: u64, name: String, traits: Option<(u8, u8, u8)>) {
        match traits.filter(|_| !name.is_empty()) {
            Some(t) => self.player_traits.insert(guid, t),
            None => self.player_traits.remove(&guid),
        };
        self.players
            .insert(guid, (!name.is_empty()).then_some(name));
        self.generation = self.generation.wrapping_add(1);
    }

    /// The reference's world-session wipe (`0x555740`; `0x5557ad call 0x560400` for the player
    /// cache): every `DBCache` whose persistence byte `[cache+0x39]` is 0 is cleared, which is
    /// `'WNAM'` (player names, ctor `0x554cd0`), `'WGLD'`, `'WPNM'` (pet names) and `'WPTN'`.
    /// Called at world entry, before the login seeds our own name.
    pub(crate) fn clear_world_session(&mut self) {
        self.players.clear();
        self.player_traits.clear();
        self.pets.clear();
        self.generation = self.generation.wrapping_add(1);
    }

    /// Install the creature templates read off disk, replacing (not merging) the current ones and
    /// touching nothing else: the load can land after the login has seeded our own name, which
    /// must survive it.
    pub(crate) fn install_persisted(&mut self, loaded: NameCache) {
        self.creatures = loaded.creatures;
        self.generation = self.generation.wrapping_add(1);
    }

    /// The landed-answer counter.
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// `(race, class, gender)` for a player guid the name query has answered for.
    pub(crate) fn player_traits(&self, guid: u64) -> Option<(u8, u8, u8)> {
        self.player_traits.get(&guid).copied()
    }

    /// Record a pet-name answer, keyed by pet number.
    pub(crate) fn insert_pet(&mut self, pet_number: u32, name: String) {
        self.pets.insert(pet_number, Some(name));
        self.generation = self.generation.wrapping_add(1);
    }

    /// Forget a pet's name and its in-flight mark so the next resolve asks again: a rename
    /// arrives only as a bumped `UNIT_FIELD_PET_NAME_TIMESTAMP`, never as the name.
    pub(crate) fn forget_pet(&mut self, pet_number: u32) {
        self.pets.evict(pet_number);
        self.generation = self.generation.wrapping_add(1);
    }

    /// Record a creature-name answer; `None` is an unknown entry.
    pub(crate) fn insert_creature(&mut self, entry: u32, record: Option<CreatureRecord>) {
        self.creatures.insert(entry, record);
        self.generation = self.generation.wrapping_add(1);
    }

    /// The cached subname, the title line under a creature's name.
    pub(crate) fn creature_subname(&self, entry: u32) -> Option<&str> {
        self.creatures.get(entry)?.subname.as_deref()
    }

    /// The cached template record. Key it by the descriptor's `OBJECT_FIELD_ENTRY`, never the
    /// guid's entry slot, as the reference does (`0x60b160`). On `None` each consumer picks its own
    /// default as the reference's getters do: `MORE_AUDIBLE` fails closed (`0x623b70`),
    /// `DO_NOT_PLAY_WOUND_ANIM` fails open (`0x6125f0`).
    pub(crate) fn creature_record(&self, entry: u32) -> Option<&CreatureRecord> {
        self.creatures.get(entry)
    }

    /// The cached `CreatureType.dbc` id; the TAB-target scan treats `None` as targetable.
    pub(crate) fn creature_type(&self, entry: u32) -> Option<u32> {
        Some(self.creatures.get(entry)?.creature_type)
    }

    /// Forget the in-flight asks a disconnect may have dropped; player and creature names stay,
    /// pet names go with the spawns they named.
    pub(crate) fn clear_pending(&mut self) {
        self.players.clear_pending();
        self.creatures.clear_pending();
        self.pets.clear();
    }
}

impl crate::query_cache::AskOnce for NameCache {
    fn clear_pending(&mut self) {
        NameCache::clear_pending(self);
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Persistence
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// The file's first line; any field that differs on load discards the whole file, the
/// reference's rule for its `.wdb` header (`[FourCC | build 0x16f3 | locale | recordSize |
/// version 1]`, no checksum, no timestamp, no TTL).
const CACHE_MAGIC: &str = "benilla-namecache";
/// Our record-layout version; bump it when a column changes. Version 2 holds creature records
/// only.
const CACHE_FORMAT: u32 = 2;
/// The client build, the reference's `0x16f3`.
const CACHE_BUILD: u32 = 5875;
/// In the header as in the reference's; benilla reads DBC locale slot 0 only.
const CACHE_LOCALE: &str = "enUS";

impl NameCache {
    /// Serialize as TSV: the header line, then one line per creature record. In-flight asks are
    /// not written.
    pub(crate) fn to_tsv(&self, realm: &str) -> String {
        let mut out =
            format!("{CACHE_MAGIC}\t{CACHE_FORMAT}\t{CACHE_BUILD}\t{CACHE_LOCALE}\t{realm}\n");
        for (entry, rec) in self.creatures.iter() {
            match rec {
                Some(r) => out.push_str(&format!(
                    "C\t{entry}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                    r.creature_type,
                    r.pet_family,
                    r.rank,
                    r.type_flags,
                    u8::from(r.civilian),
                    u8::from(r.racial_leader),
                    r.display_id,
                    r.name,
                    r.subname.as_deref().unwrap_or("")
                )),
                None => out.push_str(&format!("c\t{entry}\n")),
            }
        }
        out
    }

    /// Rebuild a cache from [`Self::to_tsv`]'s output, `None` when the header differs; a
    /// malformed line is skipped.
    pub(crate) fn from_tsv(text: &str, realm: &str) -> Option<Self> {
        let mut lines = text.lines();
        let header: Vec<&str> = lines.next()?.split('\t').collect();
        if header.len() != 5
            || header[0] != CACHE_MAGIC
            || header[1] != CACHE_FORMAT.to_string()
            || header[2] != CACHE_BUILD.to_string()
            || header[3] != CACHE_LOCALE
            || header[4] != realm
        {
            return None;
        }
        let mut cache = NameCache::default();
        for line in lines {
            let f: Vec<&str> = line.split('\t').collect();
            match f.first().copied() {
                Some("C") if f.len() >= 11 => {
                    let Ok(entry) = f[1].parse::<u32>() else {
                        continue;
                    };
                    let (Ok(creature_type), Ok(pet_family), Ok(rank), Ok(type_flags)) =
                        (f[2].parse(), f[3].parse(), f[4].parse(), f[5].parse())
                    else {
                        continue;
                    };
                    let Ok(display_id) = f[8].parse() else {
                        continue;
                    };
                    cache.creatures.insert(
                        entry,
                        Some(CreatureRecord {
                            name: f[9].to_string(),
                            // Empty is no subname, as in the wire decode.
                            subname: (!f[10].is_empty()).then(|| f[10].to_string()),
                            creature_type,
                            pet_family,
                            rank,
                            type_flags,
                            civilian: f[6] == "1",
                            racial_leader: f[7] == "1",
                            display_id,
                        }),
                    );
                }
                Some("c") if f.len() >= 2 => {
                    if let Ok(entry) = f[1].parse::<u32>() {
                        cache.creatures.insert(entry, None);
                    }
                }
                _ => {}
            }
        }
        Some(cache)
    }

    /// Drop a player's cached name and traits so the next resolve re-asks: the reference's
    /// remove-by-key for `SMSG_INVALIDATE_PLAYER` (`0x555600` into `0x560170`).
    pub(crate) fn invalidate_player(&mut self, guid: u64) {
        self.players.evict(guid);
        self.player_traits.remove(&guid);
        self.generation = self.generation.wrapping_add(1);
    }

    /// How many records the cache holds.
    pub(crate) fn len(&self) -> usize {
        self.players.len() + self.creatures.len() + self.pets.len()
    }
}

/// The three name-query answers and `SMSG_INVALIDATE_PLAYER`.
pub(crate) mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::{CreatureRecord, NameCache};
    use crate::net::NetHandlerApp;

    /// Register the four handlers.
    pub(crate) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::PlayerName, on_player_name)
            .net_handler(K::PetName, on_pet_name)
            .net_handler(K::CreatureName, on_creature_name)
            .net_handler(K::InvalidatePlayer, on_invalidate_player);
    }

    fn on_player_name(In(ev): In<SessionEvent>, mut names: ResMut<NameCache>) {
        if let SessionEvent::PlayerName {
            guid,
            name,
            race,
            class,
            gender,
        } = ev
        {
            player_name(guid, name, race, class, gender, &mut names);
        }
    }

    fn on_pet_name(In(ev): In<SessionEvent>, mut names: ResMut<NameCache>) {
        if let SessionEvent::PetName { pet_number, name } = ev {
            pet_name(pet_number, name, &mut names);
        }
    }

    fn on_creature_name(In(ev): In<SessionEvent>, mut names: ResMut<NameCache>) {
        if let SessionEvent::CreatureName {
            entry,
            name,
            subname,
            creature_type,
            pet_family,
            rank,
            type_flags,
            display_id,
            civilian,
            racial_leader,
        } = ev
        {
            creature_name(
                entry,
                name,
                subname,
                creature_type,
                pet_family,
                rank,
                type_flags,
                civilian,
                racial_leader,
                display_id,
                &mut names,
            );
        }
    }

    fn on_invalidate_player(In(ev): In<SessionEvent>, mut names: ResMut<NameCache>) {
        if let SessionEvent::InvalidatePlayer { guid } = ev {
            invalidate_player(guid, &mut names);
        }
    }

    /// `SMSG_NAME_QUERY_RESPONSE`: the name plus race, class and gender.
    fn player_name(
        guid: u64,
        name: String,
        race: u32,
        class: u32,
        gender: u32,
        names: &mut NameCache,
    ) {
        names.insert_player(guid, name, Some((race as u8, class as u8, gender as u8)));
    }

    /// `SMSG_PET_NAME_QUERY_RESPONSE`, keyed by pet number.
    fn pet_name(pet_number: u32, name: String, names: &mut NameCache) {
        names.insert_pet(pet_number, name);
    }

    /// `SMSG_INVALIDATE_PLAYER`, which vmangos sends after a rename (`CharacterHandler.cpp:839`):
    /// the reference's eviction (`0x555600`) in a name cache with no TTL; its other, an empty-name
    /// answer (`0x55f6f0`), is a cached negative here.
    fn invalidate_player(guid: u64, names: &mut NameCache) {
        debug!("net: invalidating cached name for player {guid:#x}");
        names.invalidate_player(guid);
    }

    /// `SMSG_CREATURE_QUERY_RESPONSE`; a `None` name is the server's "no such entry".
    fn creature_name(
        entry: u32,
        name: Option<String>,
        subname: Option<String>,
        creature_type: Option<u32>,
        pet_family: u32,
        rank: u32,
        type_flags: u32,
        civilian: bool,
        racial_leader: bool,
        display_id: u32,
        names: &mut NameCache,
    ) {
        names.insert_creature(
            entry,
            name.map(|n| CreatureRecord {
                name: n,
                subname,
                creature_type: creature_type.unwrap_or(0),
                pet_family,
                rank,
                type_flags,
                civilian,
                racial_leader,
                display_id,
            }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::TryRecvError;

    /// Compose a guid the way the server does: `counter | (entry << 24) | (high << 48)`.
    fn compose(high: u16, entry: u32, counter: u32) -> u64 {
        u64::from(counter) | (u64::from(entry) << 24) | (u64::from(high) << 48)
    }

    fn commands() -> (NetCommands, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        (NetCommands(tx), rx)
    }

    /// An ask must not move the counter, or an outstanding query would hold every gated feed open.
    #[test]
    fn the_generation_counts_landings_never_asks() {
        let (cmds, _rx) = commands();
        let mut cache = NameCache::default();
        let g0 = cache.generation();

        let player = compose(0, 0, 7);
        assert_eq!(cache.resolve(player, &cmds), None);
        assert_eq!(
            cache.generation(),
            g0,
            "the miss asked, and asking is not a landing"
        );

        cache.insert_player(player, "Benilla".into(), None);
        let landed = cache.generation();
        assert_ne!(g0, landed, "the answer landing is the edge");

        assert_eq!(cache.resolve(player, &cmds), Some("Benilla"));
        assert_eq!(cache.generation(), landed, "a cache hit moves nothing");

        cache.insert_pet(137, "Voidwalker".into());
        let pet = cache.generation();
        assert_ne!(landed, pet);
        cache.forget_pet(137);
        assert_ne!(
            pet,
            cache.generation(),
            "the rename eviction is a landing-shaped edge"
        );
    }

    /// A summoned pet asks `CMSG_PET_NAME_QUERY` by the pet number in its guid; its entry slot
    /// names no creature template.
    #[test]
    fn a_pet_asks_the_pet_query_not_the_creature_query() {
        let (cmds, rx) = commands();
        let mut cache = NameCache::default();
        let voidwalker = compose(guid::HIGH_PET, 137, 9);

        assert_eq!(cache.resolve(voidwalker, &cmds), None);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::PetNameQuery {
                pet_number: 137,
                ..
            })
        ));
        assert_eq!(cache.resolve(voidwalker, &cmds), None);
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));

        cache.insert_pet(137, "Voidwalker".into());
        assert_eq!(cache.resolve(voidwalker, &cmds), Some("Voidwalker"));
        assert_eq!(cache.peek(voidwalker), Some("Voidwalker"));
        // Pet names are per-spawn, so a disconnect drops them.
        cache.clear_pending();
        assert_eq!(cache.peek(voidwalker), None);
    }

    /// A stable list seeds the same `(pet_number, name)` pair the pet-name query answers, so an
    /// unstabled pet is named on arrival: `PetStable.lua:163` stores `UnitName("pet")` unguarded
    /// and `:178` hands it to `GameTooltip:SetText`, which raises on nil. vmangos packs the pet
    /// number into the guid's entry slot (`Pet.cpp:2250`).
    #[test]
    fn a_stable_list_names_the_pet_before_it_is_ever_summoned() {
        let (cmds, rx) = commands();
        let cache = NameCache::default();
        let rex = compose(guid::HIGH_PET, 7, 42);

        assert_eq!(cache.resolve(rex, &cmds), None);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::PetNameQuery { pet_number: 7, .. })
        ));

        let mut warm = NameCache::default();
        warm.insert_pet(7, "Rex".into());
        assert_eq!(warm.resolve(rex, &cmds), Some("Rex"));
        assert!(
            matches!(rx.try_recv(), Err(TryRecvError::Empty)),
            "a seeded pet must not re-ask"
        );
    }

    /// The negative answer survives too, so an unknown entry is not re-asked every login.
    #[test]
    fn the_cache_round_trips_through_its_file_format() {
        let mut cache = NameCache::default();
        cache.insert_creature(
            69,
            Some(CreatureRecord {
                name: "Stable Master Kitrik".into(),
                subname: Some("Stable Master".into()),
                creature_type: 7,
                pet_family: 0,
                rank: 1,
                type_flags: 0x10,
                civilian: true,
                racial_leader: false,
                display_id: 533,
            }),
        );
        cache.insert_creature(1234, None); // the server flagged the entry unknown

        let text = cache.to_tsv("Hydraxian Waterlords");
        let back = NameCache::from_tsv(&text, "Hydraxian Waterlords").expect("header matches");

        // Present-and-None, not absent.
        assert!(back.creatures.answered(1234));
        let rec = back.creature_record(69).expect("creature survived");
        assert_eq!(rec.name, "Stable Master Kitrik");
        assert_eq!(rec.subname.as_deref(), Some("Stable Master"));
        assert_eq!((rec.creature_type, rec.rank, rec.type_flags), (7, 1, 0x10));
        assert_eq!((rec.civilian, rec.racial_leader), (true, false));
        assert_eq!(rec.display_id, 533, "the model a stabled pet is drawn from");
        assert!(back.creature_record(1234).is_none());
        assert_eq!(back.len(), 2);

        // `Some("")` would paint an empty tooltip line.
        cache.insert_creature(
            70,
            Some(CreatureRecord {
                name: "Boar".into(),
                subname: None,
                creature_type: 1,
                pet_family: 5,
                rank: 0,
                type_flags: 0,
                civilian: false,
                racial_leader: false,
                display_id: 1,
            }),
        );
        let back = NameCache::from_tsv(&cache.to_tsv("R"), "R").expect("header");
        assert_eq!(back.creature_record(70).unwrap().subname, None);
    }

    /// The reference builds `'WNAM'` (ctor `0x554cd0`) and `'WPNM'` with persistence off; a 1.12
    /// install's `WDB/` holds `creaturecache.wdb` and no `namecache.wdb`.
    #[test]
    fn the_file_keeps_creature_templates_and_no_identities() {
        let mut cache = NameCache::default();
        cache.insert_player(0x11, "Moamtester".into(), Some((1, 2, 0)));
        cache.insert_pet(7, "Rex".into());
        cache.insert_creature(
            69,
            Some(CreatureRecord {
                name: "Stable Master Kitrik".into(),
                subname: Some("Stable Master".into()),
                creature_type: 7,
                pet_family: 0,
                rank: 1,
                type_flags: 0x10,
                civilian: true,
                racial_leader: false,
                display_id: 533,
            }),
        );

        let text = cache.to_tsv("R");
        assert!(
            !text.contains("Moamtester") && !text.contains("Rex"),
            "no guid- or spawn-keyed name may reach the disk:\n{text}"
        );
        let back = NameCache::from_tsv(&text, "R").expect("header matches");
        assert_eq!(
            back.peek(0x11),
            None,
            "a player name never survives the file"
        );
        assert_eq!(back.player_traits(0x11), None);
        assert_eq!(
            back.creature_record(69).map(|r| r.name.as_str()),
            Some("Stable Master Kitrik"),
            "the template entry is the key that keeps meaning what it meant",
        );
    }

    /// The reference's `0x555740` clears the stores whose `[cache+0x39]` byte is 0
    /// (`0x55579f`-`0x5557ad` for the player cache at `0xc0e228`).
    #[test]
    fn world_entry_clears_the_identities_and_keeps_the_templates() {
        let (cmds, rx) = commands();
        let mut cache = NameCache::default();
        cache.insert_player(0x11, "Moamtester".into(), Some((1, 2, 0)));
        cache.insert_pet(7, "Rex".into());
        cache.insert_creature(69, None);

        cache.clear_world_session();

        assert_eq!(cache.peek(0x11), None);
        assert_eq!(cache.player_traits(0x11), None);
        assert!(
            cache.creatures.answered(69),
            "templates outlive the world session"
        );
        // The in-flight marks went too.
        assert_eq!(cache.resolve(0x11, &cmds), None);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::NameQuery { guid: 0x11 })
        ));
    }

    #[test]
    fn a_name_replaced_without_traits_does_not_keep_the_old_ones() {
        let mut cache = NameCache::default();
        cache.insert_player(0x7, "Moamtester".into(), Some((1, 1, 0)));
        cache.insert_player(0x7, "Corances".into(), None);
        assert_eq!(cache.player_traits(0x7), None);
    }

    #[test]
    fn a_header_that_differs_in_any_field_discards_the_whole_file() {
        let mut cache = NameCache::default();
        cache.insert_creature(1234, None);
        let good = cache.to_tsv("Hydraxian Waterlords");
        assert!(NameCache::from_tsv(&good, "Hydraxian Waterlords").is_some());

        // The realm: our field, not the reference's.
        assert!(NameCache::from_tsv(&good, "Another Realm").is_none());

        let head = good.lines().next().unwrap();
        for (field, bad) in [
            (0, "not-benilla"),
            (1, "999"),  // format version
            (2, "5876"), // client build
            (3, "frFR"), // locale
        ] {
            let mut parts: Vec<&str> = head.split('\t').collect();
            parts[field] = bad;
            let text = format!("{}\n{}", parts.join("\t"), good.lines().nth(1).unwrap());
            assert!(
                NameCache::from_tsv(&text, "Hydraxian Waterlords").is_none(),
                "header field {field} = {bad} must discard the file"
            );
        }

        assert!(NameCache::from_tsv("", "R").is_none());
        assert!(NameCache::from_tsv("nonsense", "R").is_none());
        let text = format!("{}\nC\tnotanumber\t1\t2\t0\tX\nc\t1234\n", head);
        let back = NameCache::from_tsv(&text, "Hydraxian Waterlords").expect("header still good");
        assert_eq!(back.len(), 1, "the bad line is skipped, the good one kept");

        // A version-1 file, which held player names, is discarded whole.
        let v1 = format!(
            "{CACHE_MAGIC}\t1\t{CACHE_BUILD}\t{CACHE_LOCALE}\tHydraxian Waterlords\n\
             P\t7\t1\t1\t0\tMoamtester\n"
        );
        assert!(NameCache::from_tsv(&v1, "Hydraxian Waterlords").is_none());
    }

    #[test]
    fn invalidating_a_player_makes_the_next_resolve_ask_again() {
        let (cmds, rx) = commands();
        let mut cache = NameCache::default();
        cache.insert_player(0x11, "Aldric".into(), Some((1, 2, 0)));
        assert_eq!(cache.resolve(0x11, &cmds), Some("Aldric"));
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)), "no ask");

        let before = cache.generation();
        cache.invalidate_player(0x11);
        assert!(
            cache.generation() != before,
            "an eviction is a landed change"
        );
        assert_eq!(
            cache.player_traits(0x11),
            None,
            "the traits go with the name"
        );

        assert_eq!(cache.resolve(0x11, &cmds), None);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::NameQuery { guid: 0x11 })
        ));
    }

    #[test]
    fn creature_miss_queries_once_then_serves_the_answer() {
        let (cmds, rx) = commands();
        let mut cache = NameCache::default();
        let wolf_a = compose(guid::HIGH_UNIT, 69, 1);
        let wolf_b = compose(guid::HIGH_UNIT, 69, 2);

        assert_eq!(cache.resolve(wolf_a, &cmds), None);
        // Same entry, different spawn: no second query.
        assert_eq!(cache.resolve(wolf_b, &cmds), None);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::CreatureQuery { entry: 69, .. })
        ));
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));

        cache.insert_creature(
            69,
            Some(CreatureRecord {
                name: "Young Wolf".into(),
                subname: None,
                creature_type: 0,
                pet_family: 0,
                rank: 0,
                type_flags: 0,
                civilian: false,
                racial_leader: false,
                display_id: 0,
            }),
        );
        assert_eq!(cache.resolve(wolf_b, &cmds), Some("Young Wolf"));
    }

    #[test]
    fn player_negative_answer_is_cached() {
        let (cmds, rx) = commands();
        let mut cache = NameCache::default();
        let g = compose(guid::HIGH_PLAYER, 0, 7);

        assert_eq!(cache.resolve(g, &cmds), None);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::NameQuery { guid }) if guid == g
        ));
        cache.insert_player(g, String::new(), None); // server: unknown guid
        assert_eq!(cache.resolve(g, &cmds), None);
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn clear_pending_allows_a_reconnect_reask() {
        let (cmds, rx) = commands();
        let mut cache = NameCache::default();
        let g = compose(guid::HIGH_UNIT, 100, 1);

        assert_eq!(cache.resolve(g, &cmds), None);
        let _ = rx.try_recv();
        // A disconnect dropped the answer.
        cache.clear_pending();
        assert_eq!(cache.resolve(g, &cmds), None);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::CreatureQuery { entry: 100, .. })
        ));
    }

    /// `OBJECT_FIELD_ENTRY` and `UNIT_FIELD_PETNUMBER`, absolute descriptor indices.
    const ENTRY: u16 = 3;
    const PETNUMBER: u16 = 139;

    fn unit(fields: &[(u16, u32)]) -> ObjectStore {
        ObjectStore(benilla_protocol::ObjectFields::from_pairs(fields))
    }

    fn record(name: &str) -> CreatureRecord {
        CreatureRecord {
            name: name.into(),
            subname: None,
            creature_type: 0,
            pet_family: 0,
            rank: 0,
            type_flags: 0,
            civilian: false,
            racial_leader: false,
            display_id: 0,
        }
    }

    /// `0x609210`'s `UNIT_FIELD_PETNUMBER == 0` leg: a vmangos companion pet's guid holds a pet
    /// number the server never answers for (`PetHandler.cpp:190-192`).
    #[test]
    fn a_companion_pet_is_named_by_its_descriptor_entry() {
        let (cmds, rx) = commands();
        let mut cache = NameCache::default();
        let squirrel = compose(guid::HIGH_PET, 5_021, 3);
        let store = unit(&[(ENTRY, 2_671), (PETNUMBER, 0)]);
        cache.insert_creature(2_671, Some(record("Mechanical Squirrel")));

        assert_eq!(
            cache.resolve_unit(squirrel, Some(&store), &cmds),
            Some("Mechanical Squirrel")
        );
        assert_eq!(
            cache.peek_unit(squirrel, Some(&store)),
            Some("Mechanical Squirrel")
        );
        assert!(
            matches!(rx.try_recv(), Err(TryRecvError::Empty)),
            "no pet-name query for a unit whose PETNUMBER is 0"
        );
    }

    #[test]
    fn a_cold_companion_pet_asks_the_creature_query_for_its_entry() {
        let (cmds, rx) = commands();
        let cache = NameCache::default();
        let squirrel = compose(guid::HIGH_PET, 5_021, 3);
        let store = unit(&[(ENTRY, 2_671)]);

        assert_eq!(cache.resolve_unit(squirrel, Some(&store), &cmds), None);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::CreatureQuery { entry: 2_671, guid }) if guid == squirrel
        ));
        assert_eq!(cache.resolve_unit(squirrel, Some(&store), &cmds), None);
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn a_permanent_pet_asks_the_pet_query_for_its_descriptor_number() {
        let (cmds, rx) = commands();
        let mut cache = NameCache::default();
        let rex = compose(guid::HIGH_PET, 7, 42);
        let store = unit(&[(ENTRY, 3_122), (PETNUMBER, 7)]);
        cache.insert_creature(3_122, Some(record("Bloodtalon Taillasher")));

        assert_eq!(cache.resolve_unit(rex, Some(&store), &cmds), None);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::PetNameQuery { pet_number: 7, guid }) if guid == rex
        ));
        cache.insert_pet(7, "Rex".into());
        assert_eq!(cache.resolve_unit(rex, Some(&store), &cmds), Some("Rex"));
        assert_eq!(cache.peek_unit(rex, Some(&store)), Some("Rex"));
    }

    /// A charmed creature gets a pet number (`SpellAuras.cpp:3248`) and is named under it.
    #[test]
    fn a_charmed_creature_is_named_by_its_pet_number() {
        let (cmds, rx) = commands();
        let cache = NameCache::default();
        let ogre = compose(guid::HIGH_UNIT, 1_000, 9);
        let store = unit(&[(ENTRY, 1_000), (PETNUMBER, 55)]);

        assert_eq!(cache.resolve_unit(ogre, Some(&store), &cmds), None);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::PetNameQuery { pet_number: 55, .. })
        ));
    }
}
