//! Unit-name resolution. Descriptors carry no names: a player's answers `CMSG_NAME_QUERY` (by
//! guid), a creature's `CMSG_CREATURE_QUERY` (by template entry), a pet's or charm's
//! `CMSG_PET_NAME_QUERY` (by pet number), and which one applies is read off the descriptor as the
//! reference's `GetUnitName` (`0x609210`) reads it. Each key is asked once while in flight. A
//! refusal (a creature entry the server flagged unknown, `0x556e67`; a player's empty name,
//! `0x55f6f0`) is not an answer: its waiters read "unknown" and the next lookup that asks asks
//! again ([`crate::query_cache`]). The `resolve` reads ask; the `peek` reads never do, and are what
//! a reader that runs every frame uses: the reference's per-frame readers read the unit's cached
//! record (`[unit+0xb30]`, `0x60934b`), which only a lookup with a callback fills (`0x604660`).
//!
//! Creature templates persist, as the reference's `creaturecache.wdb` does, and a refusal never
//! does; player and pet names are never written to disk and are cleared at world-session start, as
//! the reference does.

use std::collections::HashMap;

use bevy::prelude::*;

use benilla_protocol::guid;
use benilla_protocol::messages::ObjectFields;

use crate::net::{ClientCommand, NetCommands, ObjectStore, Objects};
use crate::query_cache::{Ask, QueryCache, Refused};

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
    /// An empty wire name is a refusal: the server does not know the guid.
    players: QueryCache<u64, String>,
    /// `(race, class, gender)` from `SMSG_NAME_QUERY_RESPONSE`, the `$`-macro expander's fallback
    /// for an unstreamed subject (`0x506f70`). Present only when the server answered: a name
    /// learned otherwise (our own, seeded at login) has no entry, never a zero triple.
    player_traits: HashMap<u64, (u8, u8, u8)>,
    /// A refusal is an entry the server flagged unknown.
    creatures: QueryCache<u32, CreatureRecord>,
    /// No negative entry: the server answers for a live pet or says nothing.
    pets: QueryCache<u32, String>,
    /// Bumped by every landed answer, a refusal that took a record away and the pet-rename
    /// eviction, never by an ask: the gated feeds re-run when it moves.
    generation: u64,
    /// The creature entries in the order their records landed, for a reader that keeps its own
    /// place ([`Self::creatures_landed_since`]); a `u32` a landing, never trimmed.
    creature_landings: Vec<u32>,
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
    /// `CreatureSpellData.dbc` id of the abilities a tamed one brings (`[record+0x28]`), 0 for
    /// none: the studied-creature block's ability line (`0x529d56`).
    pub(crate) pet_spell_list: u32,
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
    /// one, the unit is not streamed: an unstreamed creature takes its entry from guid bits
    /// 24-47 (`guid::entry`), and a `HIGHGUID_PET` guid's slot is taken as the pet number, right
    /// only for a permanent pet.
    ///
    /// Deviation: the combat-log namer keys an unstreamed creature by guid bits 32-47
    /// (`0x6265c4`), which on the servers in use (entry at bits 24-47) names it wrongly.
    fn of(guid_val: u64, unit: Option<&ObjectFields>) -> Option<Self> {
        if guid::is_player(guid_val) {
            return Some(Self::Player(guid_val));
        }
        if !guid::is_creature_or_pet(guid_val) {
            return None;
        }
        match unit {
            // An entry of 0 has no key: the cache refuses it (`0x556aad`-`0x556ab6`).
            Some(unit) => match unit.unit_pet_number() {
                0 => unit.object_entry().filter(|&e| e != 0).map(Self::Creature),
                n => Some(Self::Pet(n)),
            },
            None => guid::pet_number(guid_val)
                .map(Self::Pet)
                .or_else(|| guid::entry(guid_val).map(Self::Creature)),
        }
    }
}

impl NameCache {
    /// The name for a guid; on a miss, asks once and returns `None`. The reference names a unit
    /// from its descriptor (`0x609210` reads `[unit+8]`, and `0x60b151` keys the creature cache
    /// by `[descriptor+0xc]`), so a streamed guid goes through [`Self::resolve_unit`] with its
    /// store, and only a guid that is not streamed falls to its bits ([`NameKey::of`]). A game
    /// object is `None` without a query.
    pub(crate) fn resolve(&self, objects: &Objects, guid_val: u64, ask: Ask<'_>) -> Option<&str> {
        self.resolve_unit(guid_val, objects.store(guid_val), ask)
    }

    /// The player-name cache by guid, whatever the guid is: the reference's `0x55f080` on
    /// `0xc0e228`, which names a chat speaker (`0x49d560`), the summoner (`GetSummonConfirmSummoner`, `0x48b6a0`)
    /// and every roster, mail, auction and petition guid the server sends as a player's. A guid
    /// no player owns is asked and never answered. Read as `ask` says.
    pub(crate) fn resolve_player(&self, guid_val: u64, ask: Ask<'_>) -> Option<&str> {
        self.players
            .read(guid_val, ask, |commands| {
                Self::send_player(guid_val, commands)
            })
            .map(String::as_str)
    }

    fn send_player(guid_val: u64, commands: &NetCommands) {
        debug!("names: asking player name (guid {guid_val})");
        let _ = commands.0.send(ClientCommand::NameQuery { guid: guid_val });
    }

    /// The name for a unit keyed off its descriptor, as `GetUnitName` (`0x609210`) reads it: a
    /// player's and a pet's caches are asked (`0x60925e`, `0x6092c7`), a creature's record is only
    /// read, as `[unit+0xb30]` (`0x60934b`), which the unit's create and entry change fill
    /// ([`Self::lookup_fields`], [`Self::resolve_creature`] with [`Ask::Event`]). A creature with no descriptor, which
    /// `GetUnitName` never sees, is asked by its guid bits, as the combat log's namer does
    /// (`0x6265e0`).
    pub(crate) fn resolve_unit(
        &self,
        guid_val: u64,
        unit: Option<&ObjectStore>,
        ask: Ask<'_>,
    ) -> Option<&str> {
        self.resolve_fields(guid_val, unit.map(|u| &u.0), ask)
    }

    /// [`Self::resolve_unit`] for a descriptor not yet attached to an entity, as a create block
    /// holds it.
    pub(crate) fn resolve_fields(
        &self,
        guid_val: u64,
        unit: Option<&ObjectFields>,
        ask: Ask<'_>,
    ) -> Option<&str> {
        match NameKey::of(guid_val, unit)? {
            NameKey::Player(guid_val) => self.resolve_player(guid_val, ask),
            NameKey::Pet(pet_number) => self.resolve_pet(pet_number, guid_val, ask),
            NameKey::Creature(entry) if unit.is_some() => {
                self.creatures.get(entry).map(|r| r.name.as_str())
            }
            NameKey::Creature(entry) => self.resolve_creature(entry, guid_val, ask),
        }
    }

    /// A unit's create (`0x5fb880`): every cache its name comes from looked up with a callback,
    /// a player's name by `GetUnitName` right after `PostInit` (`0x5dec09`, `0x5dec12`), and the
    /// display resolve's creature record ([`Self::ask_creature_record`]). A refusal is asked again
    /// at once, as the reference's next lookup does.
    pub(crate) fn lookup_fields(&self, guid_val: u64, unit: &ObjectFields, commands: &NetCommands) {
        if let Some(NameKey::Player(_) | NameKey::Pet(_)) = NameKey::of(guid_val, Some(unit)) {
            self.resolve_fields(guid_val, Some(unit), Ask::Event(commands));
        }
        self.ask_creature_record(guid_val, unit, commands);
    }

    /// The display resolve's ask (`0x60afb0`, at `0x60b15b`): the creature record by the
    /// descriptor's entry, for a creature or a pet alone, type mask 9 (`0x60b134`), so never for a
    /// player.
    pub(crate) fn ask_creature_record(
        &self,
        guid_val: u64,
        unit: &ObjectFields,
        commands: &NetCommands,
    ) {
        // A descriptor with no mask reads 0, which is not 9: no ask.
        if unit.object_type_mask() != Some(9) {
            return;
        }
        if let Some(entry) = unit.object_entry() {
            self.resolve_creature(entry, guid_val, Ask::Event(commands));
        }
    }

    /// There is no negative answer: vmangos sends nothing when the guid is not a live pet with
    /// that number (`PetHandler.cpp:190-192`).
    fn resolve_pet(&self, pet_number: u32, guid: u64, ask: Ask<'_>) -> Option<&str> {
        self.pets
            .read(pet_number, ask, |commands| {
                Self::send_pet(pet_number, guid, commands)
            })
            .map(String::as_str)
    }

    fn send_pet(pet_number: u32, guid: u64, commands: &NetCommands) {
        debug!("names: asking pet name (pet {pet_number})");
        let _ = commands
            .0
            .send(ClientCommand::PetNameQuery { pet_number, guid });
    }

    /// The name for a creature template `entry`; `guid` is 0 when no spawn is known, since the
    /// server answers by entry alone. Read as `ask` says.
    pub(crate) fn resolve_creature(&self, entry: u32, guid: u64, ask: Ask<'_>) -> Option<&str> {
        self.creatures
            .read(entry, ask, |commands| {
                Self::send_creature(entry, guid, commands)
            })
            .map(|r| r.name.as_str())
    }

    fn send_creature(entry: u32, guid: u64, commands: &NetCommands) {
        debug!("names: asking creature name (entry {entry})");
        let _ = commands
            .0
            .send(ClientCommand::CreatureQuery { entry, guid });
    }

    /// Record a player-name answer; an empty name is a refusal (`0x55f6f0`). `traits` is `None` for
    /// a name learned without `SMSG_NAME_QUERY_RESPONSE`, and then clears any traits filed for the
    /// guid, since they belonged to the name being replaced.
    pub(crate) fn insert_player(&mut self, guid: u64, name: String, traits: Option<(u8, u8, u8)>) {
        match traits.filter(|_| !name.is_empty()) {
            Some(t) => self.player_traits.insert(guid, t),
            None => self.player_traits.remove(&guid),
        };
        // A refusal redisplays only a name it took away: the status-0 callback `0x5fb840` does
        // nothing (`0x5fb84b`).
        let moved = if name.is_empty() {
            self.players.refuse(guid) == Refused::Record
        } else {
            self.players.insert(guid, name);
            true
        };
        if moved {
            self.generation = self.generation.wrapping_add(1);
        }
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
        let entries: Vec<u32> = self.creatures.iter().map(|(&e, _)| e).collect();
        self.creature_landings.extend(entries);
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
        self.pets.insert(pet_number, name);
        self.generation = self.generation.wrapping_add(1);
    }

    /// Forget a pet's name and its in-flight mark so the next resolve asks again: a rename
    /// arrives only as a bumped `UNIT_FIELD_PET_NAME_TIMESTAMP`, never as the name.
    pub(crate) fn forget_pet(&mut self, pet_number: u32) {
        self.pets.evict(pet_number);
        self.generation = self.generation.wrapping_add(1);
    }

    /// Record a creature-name answer; `None` is a refusal (an entry the server flagged unknown).
    /// A refusal redisplays only a record it took away: the status-0 callback `0x604660` finds
    /// no record and sets nothing (`0x60469c`).
    pub(crate) fn insert_creature(&mut self, entry: u32, record: Option<CreatureRecord>) {
        let moved = match record {
            Some(record) => {
                self.creatures.insert(entry, record);
                self.creature_landings.push(entry);
                true
            }
            None => self.creatures.refuse(entry) == Refused::Record,
        };
        if moved {
            self.generation = self.generation.wrapping_add(1);
        }
    }

    /// The creature entries that landed after a reader's place `seen`, and its new place.
    pub(crate) fn creatures_landed_since(&self, seen: usize) -> (&[u32], usize) {
        let landed = self.creature_landings.get(seen..).unwrap_or_default();
        (landed, self.creature_landings.len())
    }

    /// Every cached creature's entry and name.
    pub(crate) fn creature_names(&self) -> impl Iterator<Item = (u32, &str)> {
        self.creatures.iter().map(|(&e, r)| (e, r.name.as_str()))
    }

    /// Whether the server refused `entry` and nothing has asked for it since.
    pub(crate) fn creature_answered_unknown(&self, entry: u32) -> bool {
        self.creatures.answered_unknown(entry)
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
/// Our record-layout version; bump it when a column changes. Version 3 holds creature records
/// only, the pet spell list among them.
const CACHE_FORMAT: u32 = 3;
/// The client build, the reference's `0x16f3`.
const CACHE_BUILD: u32 = 5875;
/// The header's locale, the client's locale tag as each `.wdb` header carries it (`[0xc2a2a4]`,
/// written at `0x5567d9` and compared at `0x5573c6`): the server names creatures in the locale the
/// logon challenge sent, so a cache from another locale is discarded. Its four bytes as text,
/// `enGB`; empty for tag 0, an install with no `Wow.ini` locale.
fn cache_locale() -> String {
    let tag = crate::ui_script::client_locale().tag().to_be_bytes();
    String::from_utf8_lossy(&tag)
        .trim_start_matches('\0')
        .to_string()
}

impl NameCache {
    /// Serialize as TSV: the header line, then one line per creature record. In-flight asks and
    /// refusals are not written: the reference saves no freed node.
    pub(crate) fn to_tsv(&self, realm: &str) -> String {
        let locale = cache_locale();
        let mut out = format!("{CACHE_MAGIC}\t{CACHE_FORMAT}\t{CACHE_BUILD}\t{locale}\t{realm}\n");
        for (entry, r) in self.creatures.iter() {
            out.push_str(&format!(
                "C\t{entry}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                r.creature_type,
                r.pet_family,
                r.rank,
                r.type_flags,
                u8::from(r.civilian),
                u8::from(r.racial_leader),
                r.display_id,
                r.pet_spell_list,
                r.name,
                r.subname.as_deref().unwrap_or("")
            ));
        }
        out
    }

    /// Rebuild a cache from [`Self::to_tsv`]'s output, `None` when the header differs; a
    /// malformed line is skipped, and so is a `c` line, the negative an earlier build wrote.
    pub(crate) fn from_tsv(text: &str, realm: &str) -> Option<Self> {
        let mut lines = text.lines();
        let header: Vec<&str> = lines.next()?.split('\t').collect();
        if header.len() != 5
            || header[0] != CACHE_MAGIC
            || header[1] != CACHE_FORMAT.to_string()
            || header[2] != CACHE_BUILD.to_string()
            || header[3] != cache_locale()
            || header[4] != realm
        {
            return None;
        }
        let mut cache = NameCache::default();
        for line in lines {
            let f: Vec<&str> = line.split('\t').collect();
            match f.first().copied() {
                Some("C") if f.len() >= 12 => {
                    let Ok(entry) = f[1].parse::<u32>() else {
                        continue;
                    };
                    let (Ok(creature_type), Ok(pet_family), Ok(rank), Ok(type_flags)) =
                        (f[2].parse(), f[3].parse(), f[4].parse(), f[5].parse())
                    else {
                        continue;
                    };
                    let (Ok(display_id), Ok(pet_spell_list)) = (f[8].parse(), f[9].parse()) else {
                        continue;
                    };
                    cache.creatures.insert(
                        entry,
                        CreatureRecord {
                            name: f[10].to_string(),
                            // Empty is no subname, as in the wire decode.
                            subname: (!f[11].is_empty()).then(|| f[11].to_string()),
                            creature_type,
                            pet_family,
                            rank,
                            type_flags,
                            pet_spell_list,
                            civilian: f[6] == "1",
                            racial_leader: f[7] == "1",
                            display_id,
                        },
                    );
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
    use crate::field_watch::{id, FieldNotice};
    use crate::net::{GuidIndex, NetHandlerApp, ObjectStore};

    /// Register the four handlers.
    pub(crate) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.add_message::<FieldNotice>()
            .net_handler(K::PlayerName, on_player_name)
            .net_handler(K::PetName, on_pet_name)
            .net_handler(K::CreatureName, on_creature_name)
            .net_handler(K::InvalidatePlayer, on_invalidate_player);
    }

    /// The units the object manager holds, which an answer's callbacks name (`0x468460` with the
    /// unit mask, 8): the waiting units' own lookups.
    #[derive(bevy::ecs::system::SystemParam)]
    pub(crate) struct HeldUnits<'w, 's> {
        index: Option<Res<'w, GuidIndex>>,
        stores: Query<'w, 's, &'static ObjectStore>,
    }

    impl HeldUnits<'_, '_> {
        /// Each held unit `pick` takes, in guid order.
        fn matching(&self, pick: impl Fn(&ObjectStore) -> bool) -> Vec<u64> {
            let Some(index) = &self.index else {
                return Vec::new();
            };
            let mut out: Vec<u64> = index
                .0
                .iter()
                .filter(|&(_, &e)| self.stores.get(e).is_ok_and(|s| s.is_unit() && pick(s)))
                .map(|(&g, _)| g)
                .collect();
            out.sort_unstable();
            out
        }

        fn holds(&self, guid: u64) -> bool {
            self.index
                .as_ref()
                .and_then(|i| i.0.get(&guid))
                .and_then(|&e| self.stores.get(e).ok())
                .is_some_and(ObjectStore::is_unit)
        }
    }

    /// The name callback `0x5fb840` of a unit the answer names: for a found name (`0x5fb84b`) of a
    /// held unit (`0x5fb86a`), `UNIT_NAME_UPDATE` through `0x491a00`. The callbacks are the
    /// lookups that missed, so an answer for a name the cache already holds runs none.
    fn on_player_name(
        In(ev): In<SessionEvent>,
        mut names: ResMut<NameCache>,
        units: HeldUnits,
        mut notices: MessageWriter<FieldNotice>,
    ) {
        if let SessionEvent::PlayerName {
            guid,
            name,
            race,
            class,
            gender,
        } = ev
        {
            // A guid already cached has no lookup waiting, so no callback runs.
            let found = !name.is_empty() && names.players.get(guid).is_none();
            player_name(guid, name, race, class, gender, &mut names);
            if found && units.holds(guid) {
                notices.write(FieldNotice::Signal {
                    guid,
                    id: id::UNIT_NAME_UPDATE,
                });
            }
        }
    }

    /// The pet name cache answers through the same callback, `0x5fb840` (`0x6092b8`,
    /// `0x609307`), for each held unit whose `UNIT_FIELD_PETNUMBER` asked it.
    fn on_pet_name(
        In(ev): In<SessionEvent>,
        mut names: ResMut<NameCache>,
        units: HeldUnits,
        mut notices: MessageWriter<FieldNotice>,
    ) {
        if let SessionEvent::PetName { pet_number, name } = ev {
            let found = !name.is_empty() && names.pets.get(pet_number).is_none();
            pet_name(pet_number, name, &mut names);
            if found && pet_number != 0 {
                for guid in units.matching(|s| s.0.unit_pet_number() == pet_number) {
                    notices.write(FieldNotice::Signal {
                        guid,
                        id: id::UNIT_NAME_UPDATE,
                    });
                }
            }
        }
    }

    /// The creature cache's callback `0x604660`, for each held unit of the entry that missed it
    /// (`0x60b146`, `0x604630`): for a found record (`0x60469c`), `UNIT_NAME_UPDATE` (`0x6046d1`)
    /// then `UNIT_CLASSIFICATION_CHANGED` (`0x6046dd`).
    fn on_creature_name(
        In(ev): In<SessionEvent>,
        mut names: ResMut<NameCache>,
        units: HeldUnits,
        mut notices: MessageWriter<FieldNotice>,
    ) {
        if let SessionEvent::CreatureName {
            entry,
            name,
            subname,
            creature_type,
            pet_family,
            rank,
            type_flags,
            pet_spell_list,
            display_id,
            civilian,
            racial_leader,
        } = ev
        {
            let found = name.is_some() && names.creatures.get(entry).is_none();
            creature_name(
                entry,
                name,
                subname,
                creature_type,
                pet_family,
                rank,
                type_flags,
                pet_spell_list,
                civilian,
                racial_leader,
                display_id,
                &mut names,
            );
            if found {
                for guid in units.matching(|s| s.0.object_entry() == Some(entry)) {
                    notices.write_batch([
                        FieldNotice::Signal {
                            guid,
                            id: id::UNIT_NAME_UPDATE,
                        },
                        FieldNotice::Signal {
                            guid,
                            id: id::UNIT_CLASSIFICATION_CHANGED,
                        },
                    ]);
                }
            }
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
    /// answer (`0x55f6f0`), is a refusal ([`NameCache::insert_player`]).
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
        pet_spell_list: u32,
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
                pet_spell_list,
                civilian,
                racial_leader,
                display_id,
            }),
        );
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use benilla_protocol::messages::ObjectType;
        use benilla_protocol::ObjectFields;
        use bevy::ecs::system::RunSystemOnce;

        const PLAYER: u64 = 0x42;
        const WOLF_A: u64 = 0xF130_0000_0044_0002;
        const WOLF_B: u64 = 0xF130_0000_0044_0001;
        const BOAR: u64 = 0xF130_0000_0045_0003;
        const PET: u64 = 0xF140_0000_0000_0007;

        /// The units held: two wolves of entry 299, a boar of 300, a player and a pet numbered 9.
        fn world() -> App {
            let mut app = App::new();
            app.init_resource::<NameCache>()
                .init_resource::<GuidIndex>()
                .add_message::<FieldNotice>();
            for (guid, kind, pairs) in [
                // `OBJECT_FIELD_TYPE` (2): the unit and player type masks.
                (WOLF_A, ObjectType::Unit, vec![(2, 0x9), (3, 299)]),
                (WOLF_B, ObjectType::Unit, vec![(2, 0x9), (3, 299)]),
                (BOAR, ObjectType::Unit, vec![(2, 0x9), (3, 300)]),
                (PLAYER, ObjectType::Player, vec![(2, 0x19)]),
                (PET, ObjectType::Unit, vec![(2, 0x9), (3, 299), (139, 9)]),
            ] {
                let fields = ObjectFields::from_pairs(&pairs).into_created(kind);
                let e = app.world_mut().spawn(ObjectStore(fields)).id();
                app.world_mut()
                    .resource_mut::<GuidIndex>()
                    .0
                    .insert(guid, e);
            }
            app
        }

        fn notices(app: &mut App) -> Vec<FieldNotice> {
            app.world_mut()
                .resource_mut::<Messages<FieldNotice>>()
                .drain()
                .collect()
        }

        fn signal(guid: u64, id: u16) -> FieldNotice {
            FieldNotice::Signal { guid, id }
        }

        fn creature(entry: u32, name: Option<&str>) -> SessionEvent {
            SessionEvent::CreatureName {
                entry,
                name: name.map(str::to_string),
                subname: None,
                creature_type: None,
                pet_family: 0,
                rank: 0,
                type_flags: 0,
                pet_spell_list: 0,
                display_id: 0,
                civilian: false,
                racial_leader: false,
            }
        }

        /// A creature record names each held unit of its entry, then signals its classification;
        /// a miss signals nothing.
        #[test]
        fn a_creature_answer_signals_each_held_unit_of_its_entry() {
            let mut app = world();
            app.world_mut()
                .run_system_once_with(on_creature_name, creature(299, Some("Wolf")))
                .unwrap();
            assert_eq!(
                notices(&mut app),
                [
                    signal(WOLF_B, 183),
                    signal(WOLF_B, 187),
                    signal(WOLF_A, 183),
                    signal(WOLF_A, 187),
                    signal(PET, 183),
                    signal(PET, 187)
                ]
            );
            app.world_mut()
                .run_system_once_with(on_creature_name, creature(300, None))
                .unwrap();
            assert!(notices(&mut app).is_empty(), "no such entry");
            app.world_mut()
                .run_system_once_with(on_creature_name, creature(299, Some("Wolf")))
                .unwrap();
            assert!(
                notices(&mut app).is_empty(),
                "a repeat finds no lookup waiting"
            );
        }

        /// A found player name signals the held player alone; an empty one, or a guid not held,
        /// nothing. A pet name signals the held unit its pet number names.
        #[test]
        fn a_name_answer_signals_the_held_unit_it_names() {
            let mut app = world();
            let player = |guid: u64, name: &str| SessionEvent::PlayerName {
                guid,
                name: name.into(),
                race: 1,
                gender: 0,
                class: 1,
            };
            app.world_mut()
                .run_system_once_with(on_player_name, player(PLAYER, "Brisca"))
                .unwrap();
            assert_eq!(notices(&mut app), [signal(PLAYER, 183)]);
            app.world_mut()
                .run_system_once_with(on_player_name, player(PLAYER, "Brisca"))
                .unwrap();
            assert!(
                notices(&mut app).is_empty(),
                "a repeat finds no lookup waiting"
            );
            app.world_mut()
                .run_system_once_with(on_player_name, player(0x43, "Far"))
                .unwrap();
            app.world_mut()
                .run_system_once_with(on_player_name, player(PLAYER, ""))
                .unwrap();
            assert!(notices(&mut app).is_empty());
            app.world_mut()
                .run_system_once_with(
                    on_pet_name,
                    SessionEvent::PetName {
                        pet_number: 9,
                        name: "Fang".into(),
                    },
                )
                .unwrap();
            assert_eq!(notices(&mut app), [signal(PET, 183)]);
            app.world_mut()
                .run_system_once_with(
                    on_pet_name,
                    SessionEvent::PetName {
                        pet_number: 9,
                        name: "Fang".into(),
                    },
                )
                .unwrap();
            assert!(notices(&mut app).is_empty(), "a repeat pet name");
        }
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
        assert_eq!(cache.resolve_unit(player, None, Ask::New(&cmds)), None);
        assert_eq!(
            cache.generation(),
            g0,
            "the miss asked, and asking is not a landing"
        );

        cache.insert_player(player, "Benilla".into(), None);
        let landed = cache.generation();
        assert_ne!(g0, landed, "the answer landing is the edge");

        assert_eq!(
            cache.resolve_unit(player, None, Ask::New(&cmds)),
            Some("Benilla")
        );
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

        assert_eq!(cache.resolve_unit(voidwalker, None, Ask::New(&cmds)), None);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::PetNameQuery {
                pet_number: 137,
                ..
            })
        ));
        assert_eq!(cache.resolve_unit(voidwalker, None, Ask::New(&cmds)), None);
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));

        cache.insert_pet(137, "Voidwalker".into());
        assert_eq!(
            cache.resolve_unit(voidwalker, None, Ask::New(&cmds)),
            Some("Voidwalker")
        );
        assert_eq!(
            cache.resolve_unit(voidwalker, None, Ask::Read),
            Some("Voidwalker")
        );
        // Pet names are per-spawn, so a disconnect drops them.
        cache.clear_pending();
        assert_eq!(cache.resolve_unit(voidwalker, None, Ask::Read), None);
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

        assert_eq!(cache.resolve_unit(rex, None, Ask::New(&cmds)), None);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::PetNameQuery { pet_number: 7, .. })
        ));

        let mut warm = NameCache::default();
        warm.insert_pet(7, "Rex".into());
        assert_eq!(warm.resolve_unit(rex, None, Ask::New(&cmds)), Some("Rex"));
        assert!(
            matches!(rx.try_recv(), Err(TryRecvError::Empty)),
            "a seeded pet must not re-ask"
        );
    }

    /// A refusal is not written, so an unknown entry is asked again at the next login.
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
                pet_spell_list: 5796,
                civilian: true,
                racial_leader: false,
                display_id: 533,
            }),
        );
        cache.insert_creature(1234, None); // the server flagged the entry unknown

        let text = cache.to_tsv("Hydraxian Waterlords");
        assert!(
            !text.lines().any(|l| l.split('\t').nth(1) == Some("1234")),
            "a refusal is not a line of the file"
        );
        let back = NameCache::from_tsv(&text, "Hydraxian Waterlords").expect("header matches");

        assert!(
            !back.creatures.answered(1234),
            "and so it does not come back"
        );
        let rec = back.creature_record(69).expect("creature survived");
        assert_eq!(rec.name, "Stable Master Kitrik");
        assert_eq!(rec.subname.as_deref(), Some("Stable Master"));
        assert_eq!((rec.creature_type, rec.rank, rec.type_flags), (7, 1, 0x10));
        assert_eq!((rec.civilian, rec.racial_leader), (true, false));
        assert_eq!(rec.display_id, 533, "the model a stabled pet is drawn from");
        assert_eq!(rec.pet_spell_list, 5796, "the studied block's abilities");
        assert!(back.creature_record(1234).is_none());
        assert_eq!(back.len(), 1);

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
                pet_spell_list: 0,
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
                pet_spell_list: 0,
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
            back.resolve_unit(0x11, None, Ask::Read),
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
        cache.insert_creature(69, Some(record("Stable Master")));

        cache.clear_world_session();

        assert_eq!(cache.resolve_unit(0x11, None, Ask::Read), None);
        assert_eq!(cache.player_traits(0x11), None);
        assert!(
            cache.creatures.get(69).is_some(),
            "templates outlive the world session"
        );
        // The in-flight marks went too.
        assert_eq!(cache.resolve_unit(0x11, None, Ask::New(&cmds)), None);
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
        cache.insert_creature(1234, Some(record("Defias Thug")));
        let good = cache.to_tsv("Hydraxian Waterlords");
        assert!(NameCache::from_tsv(&good, "Hydraxian Waterlords").is_some());

        // The realm: our field, not the reference's.
        assert!(NameCache::from_tsv(&good, "Another Realm").is_none());

        // The locale field is the client's tag, as the reference's `.wdb` header holds it.
        let tag = crate::ui_script::client_locale().tag().to_be_bytes();
        let spelled = String::from_utf8_lossy(&tag)
            .trim_start_matches('\0')
            .to_string();
        assert_eq!(
            good.lines().next().unwrap().split('\t').nth(3),
            Some(&*spelled)
        );

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
        let text = format!(
            "{}\nC\tnotanumber\t1\t2\t0\tX\nc\t1234\nC\t88\t7\t0\t1\t0\t0\t0\t5\t0\tGuard\t\n",
            head
        );
        let back = NameCache::from_tsv(&text, "Hydraxian Waterlords").expect("header still good");
        assert_eq!(back.len(), 1, "the bad line is skipped, the good one kept");
        assert!(back.creature_record(88).is_some());

        // A version-1 file, which held player names, is discarded whole.
        let v1 = format!(
            "{CACHE_MAGIC}\t1\t{CACHE_BUILD}\t{}\tHydraxian Waterlords\n\
             P\t7\t1\t1\t0\tMoamtester\n",
            cache_locale()
        );
        assert!(NameCache::from_tsv(&v1, "Hydraxian Waterlords").is_none());
    }

    #[test]
    fn invalidating_a_player_makes_the_next_resolve_ask_again() {
        let (cmds, rx) = commands();
        let mut cache = NameCache::default();
        cache.insert_player(0x11, "Aldric".into(), Some((1, 2, 0)));
        assert_eq!(
            cache.resolve_unit(0x11, None, Ask::New(&cmds)),
            Some("Aldric")
        );
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

        assert_eq!(cache.resolve_unit(0x11, None, Ask::New(&cmds)), None);
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

        assert_eq!(cache.resolve_unit(wolf_a, None, Ask::New(&cmds)), None);
        // Same entry, different spawn: no second query.
        assert_eq!(cache.resolve_unit(wolf_b, None, Ask::New(&cmds)), None);
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
                pet_spell_list: 0,
                civilian: false,
                racial_leader: false,
                display_id: 0,
            }),
        );
        assert_eq!(
            cache.resolve_unit(wolf_b, None, Ask::New(&cmds)),
            Some("Young Wolf")
        );
    }

    /// `0x55f6f0`: an empty name runs the callbacks with status 0 and frees the node. A reader's
    /// ask within the window from the refusal sends nothing; the player's next create asks at
    /// once, as the reference's next lookup with a callback does.
    #[test]
    fn an_empty_player_name_is_a_refusal_and_the_next_lookup_asks_again() {
        let (cmds, rx) = commands();
        let mut cache = NameCache::default();
        let g = compose(guid::HIGH_PLAYER, 0, 7);
        let fields = ObjectFields::from_pairs(&[]);

        cache.lookup_fields(g, &fields, &cmds);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::NameQuery { guid }) if guid == g
        ));
        let before = cache.generation();
        cache.insert_player(g, String::new(), None); // server: unknown guid
        assert!(
            cache.players.answered_unknown(g),
            "the waiters read unknown"
        );
        assert_eq!(
            cache.generation(),
            before,
            "nothing redisplays: `0x5fb840` ignores status 0"
        );
        assert_eq!(cache.resolve_unit(g, None, Ask::New(&cmds)), None);
        assert!(
            matches!(rx.try_recv(), Err(TryRecvError::Empty)),
            "a reader's ask within the window does not send"
        );

        cache.lookup_fields(g, &fields, &cmds);
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::NameQuery { guid }) if guid == g),
            "the next create does"
        );
    }

    /// `0x556e67`: a creature key with its top bit set is a refusal; the template is not kept.
    /// A unit of it in view reads its name every frame and never asks (`[unit+0xb30]`,
    /// `0x60934b`); the next unit of that entry created asks at once (`0x60b15b`).
    #[test]
    fn a_refused_creature_is_asked_again_by_the_next_unit_created_not_by_the_readers() {
        let (cmds, rx) = commands();
        let mut cache = NameCache::default();
        let wolf_a = compose(guid::HIGH_UNIT, 1234, 1);
        let wolf_b = compose(guid::HIGH_UNIT, 1234, 2);
        let store = unit(&[(TYPE, CREATURE), (ENTRY, 1234)]);

        cache.lookup_fields(wolf_a, &store.0, &cmds);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::CreatureQuery { entry: 1234, .. })
        ));
        cache.insert_creature(1234, None);
        assert!(cache.creatures.answered_unknown(1234));
        assert!(cache.creature_record(1234).is_none());
        for _ in 0..600 {
            assert_eq!(
                cache.resolve_unit(wolf_a, Some(&store), Ask::New(&cmds)),
                None
            );
            assert_eq!(cache.resolve_unit(wolf_a, Some(&store), Ask::Read), None);
        }
        assert!(
            matches!(rx.try_recv(), Err(TryRecvError::Empty)),
            "the readers of a unit in view never ask"
        );

        cache.lookup_fields(wolf_b, &store.0, &cmds);
        assert!(
            matches!(
                rx.try_recv(),
                Ok(ClientCommand::CreatureQuery { entry: 1234, guid }) if guid == wolf_b
            ),
            "a new unit of the entry asks at once"
        );
        cache.lookup_fields(wolf_b, &store.0, &cmds);
        assert!(
            matches!(rx.try_recv(), Err(TryRecvError::Empty)),
            "and never while that ask is pending"
        );
    }

    /// `0x556ec9`: a refusal for an entry nobody asked for is no answer at all.
    #[test]
    fn a_refusal_nobody_asked_for_marks_nothing() {
        let mut cache = NameCache::default();
        let before = cache.generation();
        cache.insert_creature(4321, None);
        assert!(!cache.creatures.answered_unknown(4321));
        assert_eq!(cache.generation(), before);
    }

    /// `0x556f35` frees a valid node too, and what the readers showed changes.
    #[test]
    fn a_refusal_of_a_held_record_takes_it_away_and_redisplays() {
        let mut cache = NameCache::default();
        cache.insert_creature(55, Some(record("Kobold")));
        let before = cache.generation();
        cache.insert_creature(55, None);
        assert!(cache.creature_record(55).is_none());
        assert_ne!(cache.generation(), before);
    }

    /// An earlier build wrote its refusals as `c` lines. They load as nothing, and the records
    /// around them load.
    #[test]
    fn a_file_with_negative_lines_loads_without_them() {
        let mut cache = NameCache::default();
        cache.insert_creature(69, Some(record("Stable Master")));
        let written = cache.to_tsv("R");
        let (header, rest) = written.split_once('\n').expect("a header line");
        let old = format!("{header}\nc\t1234\n{rest}c\t77\n");

        let back = NameCache::from_tsv(&old, "R").expect("the header is this build's");
        assert!(!back.creatures.answered(1234), "a negative line is dropped");
        assert!(!back.creatures.answered(77));
        assert_eq!(back.len(), 1);
        assert_eq!(
            back.creature_record(69).map(|r| r.name.as_str()),
            Some("Stable Master")
        );
        assert!(
            !back.to_tsv("R").contains("1234"),
            "and the next save does not bring it back"
        );
    }

    #[test]
    fn clear_pending_allows_a_reconnect_reask() {
        let (cmds, rx) = commands();
        let mut cache = NameCache::default();
        let g = compose(guid::HIGH_UNIT, 100, 1);

        assert_eq!(cache.resolve_unit(g, None, Ask::New(&cmds)), None);
        let _ = rx.try_recv();
        // A disconnect dropped the answer.
        cache.clear_pending();
        assert_eq!(cache.resolve_unit(g, None, Ask::New(&cmds)), None);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::CreatureQuery { entry: 100, .. })
        ));
    }

    /// `OBJECT_FIELD_ENTRY` and `UNIT_FIELD_PETNUMBER`, absolute descriptor indices.
    const ENTRY: u16 = 3;
    const PETNUMBER: u16 = 139;
    /// `OBJECT_FIELD_TYPE` and a creature's or a pet's mask, which the display resolve's ask
    /// needs (`0x60b134`).
    const TYPE: u16 = 2;
    const CREATURE: u32 = 9;

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
            pet_spell_list: 0,
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
            cache.resolve_unit(squirrel, Some(&store), Ask::New(&cmds)),
            Some("Mechanical Squirrel")
        );
        assert_eq!(
            cache.resolve_unit(squirrel, Some(&store), Ask::Read),
            Some("Mechanical Squirrel")
        );
        assert!(
            matches!(rx.try_recv(), Err(TryRecvError::Empty)),
            "no pet-name query for a unit whose PETNUMBER is 0"
        );
    }

    /// `[[unit+8]+0xc]` keys the creature cache (`0x604629`): a creature whose guid bits spell
    /// another entry is named, and asked for, by its descriptor, and a descriptor with no entry
    /// yet has no key (the cache refuses 0, `0x556aad`-`0x556ab6`, and `0x604646` skips the store).
    #[test]
    fn a_creature_is_keyed_by_its_descriptor_entry_not_its_guid_bits() {
        let (cmds, rx) = commands();
        let mut cache = NameCache::default();
        let wolf = compose(guid::HIGH_UNIT, 299, 4);
        let store = unit(&[(TYPE, CREATURE), (ENTRY, 2_671)]);
        cache.insert_creature(299, Some(record("Guid Bits")));
        cache.insert_creature(2_671, Some(record("Field Entry")));
        assert_eq!(
            cache.resolve_unit(wolf, Some(&store), Ask::New(&cmds)),
            Some("Field Entry")
        );

        let cold = NameCache::default();
        cold.lookup_fields(wolf, &store.0, &cmds);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::CreatureQuery { entry: 2_671, .. })
        ));

        let bare = unit(&[]);
        let fresh = NameCache::default();
        fresh.lookup_fields(wolf, &bare.0, &cmds);
        assert_eq!(fresh.resolve_unit(wolf, Some(&bare), Ask::New(&cmds)), None);
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }

    /// `resolve` finds the streamed object and names it from its descriptor (`0x609210`), so a
    /// creature whose guid bits spell another entry is named by the field entry, and read, never
    /// asked (`[unit+0xb30]`, `0x60934b`); only an unstreamed guid falls to its bits, and is asked.
    #[test]
    fn resolve_names_a_streamed_unit_by_its_descriptor_not_its_guid_bits() {
        let (cmds, rx) = commands();
        let wolf = compose(guid::HIGH_UNIT, 299, 4);
        let mut objs = crate::ui_items::TestObjects::new();
        objs.spawn_object(
            wolf,
            ObjectFields::from_pairs(&[(ENTRY, 2_671), (PETNUMBER, 0)]),
        );
        let objects = objs.get();

        let cold = NameCache::default();
        assert_eq!(cold.resolve(&objects, wolf, Ask::New(&cmds)), None);
        assert!(
            matches!(rx.try_recv(), Err(TryRecvError::Empty)),
            "a streamed creature is read, not asked"
        );
        let stranger = compose(guid::HIGH_UNIT, 299, 5);
        assert_eq!(cold.resolve(&objects, stranger, Ask::New(&cmds)), None);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::CreatureQuery { entry: 299, guid }) if guid == stranger
        ));

        let mut warm = NameCache::default();
        warm.insert_creature(299, Some(record("Guid Bits")));
        warm.insert_creature(2_671, Some(record("Field Entry")));
        assert_eq!(
            warm.resolve(&objects, wolf, Ask::New(&cmds)),
            Some("Field Entry")
        );

        // Not streamed: the bits are all there is.
        assert_eq!(
            warm.resolve(&objects, stranger, Ask::New(&cmds)),
            Some("Guid Bits")
        );
    }

    /// `0x55f080` on `0xc0e228` takes the guid as it is: a chat speaker or summoner that is not a
    /// player is asked of the player cache, and never of the creature cache by its bits.
    #[test]
    fn resolve_player_asks_the_player_cache_for_any_guid() {
        let (cmds, rx) = commands();
        let cache = NameCache::default();
        let creature = compose(guid::HIGH_UNIT, 299, 4);
        assert_eq!(cache.resolve_player(creature, Ask::New(&cmds)), None);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::NameQuery { guid }) if guid == creature
        ));
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }

    /// The display resolve asks only for mask 9 (`0x60b134`): a descriptor with no
    /// `OBJECT_FIELD_TYPE` reads 0 and asks nothing, nor does a player's `0x19`.
    #[test]
    fn only_a_creatures_mask_asks_its_record() {
        let (cmds, rx) = commands();
        let cache = NameCache::default();
        let wolf = compose(guid::HIGH_UNIT, 1234, 1);
        cache.ask_creature_record(wolf, &unit(&[(ENTRY, 1234)]).0, &cmds);
        cache.ask_creature_record(wolf, &unit(&[(TYPE, 0x19), (ENTRY, 1234)]).0, &cmds);
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
        cache.ask_creature_record(wolf, &unit(&[(TYPE, CREATURE), (ENTRY, 1234)]).0, &cmds);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::CreatureQuery { entry: 1234, .. })
        ));
    }

    #[test]
    fn a_cold_companion_pet_is_asked_by_the_creature_query_at_its_create() {
        let (cmds, rx) = commands();
        let cache = NameCache::default();
        let squirrel = compose(guid::HIGH_PET, 5_021, 3);
        let store = unit(&[(TYPE, CREATURE), (ENTRY, 2_671)]);

        cache.lookup_fields(squirrel, &store.0, &cmds);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::CreatureQuery { entry: 2_671, guid }) if guid == squirrel
        ));
        assert!(
            matches!(rx.try_recv(), Err(TryRecvError::Empty)),
            "no pet-name query for a PETNUMBER of 0"
        );
        assert_eq!(
            cache.resolve_unit(squirrel, Some(&store), Ask::New(&cmds)),
            None
        );
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn a_permanent_pet_asks_the_pet_query_for_its_descriptor_number() {
        let (cmds, rx) = commands();
        let mut cache = NameCache::default();
        let rex = compose(guid::HIGH_PET, 7, 42);
        let store = unit(&[(ENTRY, 3_122), (PETNUMBER, 7)]);
        cache.insert_creature(3_122, Some(record("Bloodtalon Taillasher")));

        assert_eq!(cache.resolve_unit(rex, Some(&store), Ask::New(&cmds)), None);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::PetNameQuery { pet_number: 7, guid }) if guid == rex
        ));
        cache.insert_pet(7, "Rex".into());
        assert_eq!(
            cache.resolve_unit(rex, Some(&store), Ask::New(&cmds)),
            Some("Rex")
        );
        assert_eq!(
            cache.resolve_unit(rex, Some(&store), Ask::Read),
            Some("Rex")
        );
    }

    /// A charmed creature gets a pet number (`SpellAuras.cpp:3248`) and is named under it.
    #[test]
    fn a_charmed_creature_is_named_by_its_pet_number() {
        let (cmds, rx) = commands();
        let cache = NameCache::default();
        let ogre = compose(guid::HIGH_UNIT, 1_000, 9);
        let store = unit(&[(ENTRY, 1_000), (PETNUMBER, 55)]);

        assert_eq!(
            cache.resolve_unit(ogre, Some(&store), Ask::New(&cmds)),
            None
        );
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::PetNameQuery { pet_number: 55, .. })
        ));
    }
}
