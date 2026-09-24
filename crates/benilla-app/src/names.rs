//! Unit-name resolution — the query-cache seam of decision 0068 §3.
//!
//! The 1.12 wire carries **no names in descriptors**: a player's name answers `CMSG_NAME_QUERY`
//! (keyed by guid), a creature's answers `CMSG_CREATURE_QUERY` (keyed by the template *entry*,
//! shared by every spawn of that template), a pet's or charm's answers `CMSG_PET_NAME_QUERY`
//! (keyed by its pet number). Which of the three names a streamed unit is read off its
//! **descriptor**, as the reference's `GetUnitName` (`0x609210`) reads it — see [`NameKey`]. This
//! module owns the cache and the **ask-once** discipline: a consumer calls
//! [`NameCache::resolve_unit`] (or [`NameCache::resolve`] with only a guid), which returns the name
//! when known and otherwise issues the query (deduped while in flight) and reports "not yet". The net bridge ([`crate::net`]) fills the cache from the
//! decoded `PlayerName`/`CreatureName` events and clears the in-flight sets on disconnect (a query
//! dropped by a dead writer must be re-askable after reconnect).
//!
//! A *negative* answer (the server doesn't know the guid/entry) is cached too — resolving to
//! "unknown, and asking again won't help" — so a bad id can never turn into a query loop.
//!
//! **Two of the three live for one world session; one persists.** Creature templates are keyed by
//! a template entry that means the same thing forever, and the reference writes them to
//! `creaturecache.wdb` — so ours persist too (decision 1689, corrected scope). Player names and
//! pet names are keyed by a guid and a pet number, which name *one character* and *one spawn*, and
//! the reference neither writes them to disk nor carries them across a world entry — it clears
//! both stores at world-session start ([`NameCache::clear_world_session`]). Persisting them was
//! benilla's own addition, and it is what answered a wiped server's brand-new character with a
//! deleted one's name (report B386, decision 2223).

use std::collections::HashMap;

use bevy::prelude::*;

use benilla_protocol::guid;

use crate::net::{ClientCommand, NetCommands, ObjectStore};
use crate::query_cache::QueryCache;

/// The creature template's **`type_flags`** word, bit by bit — the dword the server ships in
/// `SMSG_CREATURE_QUERY_RESPONSE` and the reference caches at `[CGUnit+0xb30] + 0x14` (its async
/// creature-query record, **not** a DBC row and **not** `CreatureType.dbc`).
///
/// Named because the bits are one-apart neighbours with wholly unrelated meanings, and reading a
/// literal `& 0x10` at a call site is how the wound gate below spent months testing the faction
/// tooltip's bit instead of its own (decision 2068). The reference gives each bit its own getter,
/// which is the shape mirrored here — every consumer names the bit it wants:
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
/// (bit numbering per the readers above; the names corroborated by vmangos
/// `CreatureDefines.h:146-152`.)
pub(crate) mod type_flags {
    /// `0x8` — **DO_NOT_PLAY_WOUND_ANIM**: this creature has nothing to flinch with. The
    /// reference refuses the victim wound-flinch overlay outright for it (`0x60ea9f` inside the
    /// flinch `0x60ea70`), whatever the trigger — a skeleton, a ghost, an elemental takes hits
    /// without recoiling. It gates **only** the animation: the blood spurt is unaffected
    /// (the reference has no blood-, creature-type- or display-record-keyed mechanism that
    /// suppresses it — a skeleton bleeds), and so is the floating combat text. Decision 2068.
    pub(crate) const DO_NOT_PLAY_WOUND_ANIM: u32 = 0x8;

    /// `0x10` — **NO_FACTION_TOOLTIP**: the unit tooltip drops its faction-name line
    /// (the reference's `0x612610`, which returns the bit inverted).
    pub(crate) const NO_FACTION_TOOLTIP: u32 = 0x10;

    /// `0x20` — **MORE_AUDIBLE**: the pass-2 election re-links an off-screen creature for
    /// tick-only so its combat stays audible (`0x607da0`'s `0x623b70` arm, decision 1482).
    pub(crate) const MORE_AUDIBLE: u32 = 0x20;
}

/// The name cache: players by guid, creatures by template entry, plus the in-flight ask-once sets.
/// Filled by the net bridge; read (and query-triggered) through [`Self::resolve`].
#[derive(Resource, Default)]
pub(crate) struct NameCache {
    /// Player names by guid. `Some(None)`-shaped answers are stored as `None`: the server was asked
    /// and didn't know (an empty wire name) — cached so we never re-ask a dead guid.
    players: QueryCache<u64, String>,
    /// `(race, class, gender)` from the same `SMSG_NAME_QUERY_RESPONSE` that carried the name — the
    /// wire has always sent these three and we used to drop them. Two consumers now: the
    /// `$`-macro expander's non-player-subject path: the reference resolves a macro subject from
    /// the object manager first and falls back to **this** cache record when the unit isn't
    /// streamed (`0x506f70`), so without them `$R`/`$C`/`$G` against an
    /// off-screen player would silently read race 0.
    ///
    /// **An entry here is present only when the server answered for it.** A name learned any other
    /// way (our own, seeded from the login) has no entry, and that absence is the live shape
    /// decision 1549 rests on — not a gap to fill with a zero triple.
    player_traits: HashMap<u64, (u8, u8, u8)>,
    /// Creature template records by entry; the outer `None` = the server flagged the entry
    /// unknown. The subname is the overhead/tooltip title line ("Stable Master", …); the type is
    /// the `CreatureType.dbc` id the TAB-target critter filter reads; rank/civilian feed the
    /// unit tooltip's level-line word + CIVILIAN line (decision 0276).
    creatures: QueryCache<u32, CreatureRecord>,
    /// Pet names by **pet number** — the third naming path (see [`Self::resolve`]'s pet branch).
    /// No negative entry exists: the server answers for a live pet or says nothing at all, so an
    /// unanswered ask stays unanswered, exactly as it does for the real client.
    pets: QueryCache<u32, String>,
    /// Bumped by every **landed** answer (and the pet-rename eviction) — never by an ask. The
    /// gated feeds' watch counter (decision 1439): a feed that resolved a miss re-runs when the
    /// answer lands, and only then. Born because `is_changed` could not say this while the
    /// resolves took `&mut self` every frame; 2288 made them `&self` (the in-flight marks live
    /// behind [`QueryCache`]'s lock), and the counter stays as the one number the three caches
    /// share.
    generation: u64,
}

/// One cached creature template head (see [`NameCache::creatures`]).
#[derive(Clone, Debug)]
pub(crate) struct CreatureRecord {
    pub(crate) name: String,
    pub(crate) subname: Option<String>,
    pub(crate) creature_type: u32,
    /// `CreatureFamily.dbc` id — the pet paper doll's family word and, through the row's food
    /// mask, its diet tooltip (decision 1062). `0` on everything that is neither a tameable beast
    /// nor a warlock minion, which is most of the table; the family table has no row 0, so a `0`
    /// resolves to nil without needing a sentinel of its own.
    pub(crate) pet_family: u32,
    /// Elite rank 0..4 **as the template declares it** — read it through [`gated_rank`], never
    /// directly, unless you specifically want the ungated template value.
    pub(crate) rank: u32,
    /// Template type flags — the wire dword, read bit by bit through [`type_flags`]. Never test a
    /// literal against it: its bits are unrelated neighbours (decision 2068).
    pub(crate) type_flags: u32,
    pub(crate) civilian: bool,
    /// Racial leader — the tooltip's white LEADER line (`0x6125c0`).
    pub(crate) racial_leader: bool,
    /// `CreatureDisplayInfo.dbc` id — the template's model. The **only** way to draw a creature
    /// with no world object to read `UNIT_FIELD_DISPLAYID` off, which is exactly a stabled pet
    /// (decision 1676): its wire row names a template entry and nothing else. `0` when the
    /// template ships none.
    ///
    /// For anything actually on screen the unit's own descriptor field still wins — vmangos picks
    /// among a template's four display ids at spawn, and this is the first.
    ///
    /// **Read by the stable window's model pane** ([`crate::ui_stable`]'s `feed_stable_booth`),
    /// which is what 1676 deferred and named this field for: the booth points at a
    /// [`crate::portrait::PortraitStandIn`] built from this id when the selected pet has no world
    /// object — every stabled pet, and a dismissed one in slot 0.
    pub(crate) display_id: u32,
}

/// The client's **creature-rank getter**, `0x605620` — 33 bytes, and the single source every rank
/// reader in the real client goes through (decision 0782):
///
/// ```text
/// 605620: mov eax,[ecx+0xb30]   ; the cached creature record
///         test eax,eax / je  →  0     ; not queried yet
///         mov ecx,[ecx+0x110]         ; the unit descriptor block
///         mov edx,[ecx+0x214]         ; UNIT_FIELD_PETNUMBER
///         test edx,edx / jne →  0     ; somebody's pet or charm
///         mov eax,[eax+0x20]          ; record->rank
/// ```
///
/// Two gates, and the pet gate is the one nobody expects: a **charmed or enslaved unit reports rank
/// 0** whatever its template says. Because all three of the client's rank consumers call this one
/// getter — `UnitClassification` (the target frame's elite/rare border), the unit tooltip's
/// ELITE/BOSS word, and `UnitLevel`'s world-boss `−1` (the target frame *and* nameplate skull) — a
/// mind-controlled world boss loses its dragon, its BOSS word and its skull together. Applying the
/// gate per-reader is how those three silently drift apart, so this function is the only place any
/// of them may read a rank from.
///
/// `store: None` (no descriptor streamed) cannot prove a pet number, so it reads as not-a-pet —
/// matching the client, whose descriptor block is zero-initialized.
pub(crate) fn gated_rank(rec: Option<&CreatureRecord>, store: Option<&ObjectStore>) -> u32 {
    match rec {
        Some(rec) if !store.is_some_and(|s| s.0.unit_is_pet_or_charm()) => rec.rank,
        _ => 0,
    }
}

/// Which of the three caches names a unit, and under which key — the branch of the reference's
/// `CGUnit_C::GetUnitName` (`0x609210`), which is also what the combat log's `GetObjectName`
/// (`0x6264e0`) calls for any streamed unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NameKey {
    /// A player — the `OBJECT_FIELD_TYPE` player bit (`0x609232 shr ecx,4; test cl,1`): the
    /// name cache by guid.
    Player(u64),
    /// `UNIT_FIELD_PETNUMBER` non-zero (`0x609295`): the pet-name cache under that number.
    Pet(u32),
    /// `UNIT_FIELD_PETNUMBER` zero (`0x60929d je 0x60934b`): the creature record at `[unit+0xb30]`,
    /// which is registered under the descriptor's `OBJECT_FIELD_ENTRY` (`0x60b160`).
    Creature(u32),
}

impl NameKey {
    /// The key for `guid`, read off its descriptor when one is in hand.
    ///
    /// **With a descriptor this is `0x609210` exactly**: the pet number and the template entry are
    /// the unit's *fields*, never the guid's bits. The two disagree more often than they agree
    /// outside a hunter's or warlock's own pet — vmangos gives a companion pet a `HIGHGUID_PET`
    /// guid around a fresh pet number but never files it in the charm info (so `PETNUMBER` is 0 and
    /// the server would ignore a pet-name query for it); a guardian's `SetPetNumber(n, false)`
    /// writes 0; a mind-controlled creature keeps a `HIGHGUID_UNIT` guid and gains a non-zero
    /// number.
    ///
    /// **Without one it is the best the guid can say**, and says less: a `HIGHGUID_PET` guid's
    /// entry slot is taken as the pet number, which is right for a permanent pet (`Pet::Create`
    /// feeds the same number to both — `SetPetNumber(n, IsPermanentPetFor(owner))`) and cannot name
    /// a companion or a guardian. A caller that holds the unit's store must pass it.
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
    /// The name for `guid` **when no descriptor for it is in hand**, if known — a roster member, a
    /// mail sender, a chat speaker, a unit that has left. On a miss, sends the right query (once
    /// per key per connection) and returns `None` — call again after the answer lands. A guid
    /// family that has no name on the 1.12 wire (GameObjects resolve via their own query, not
    /// modeled yet) is `None` without a query.
    ///
    /// **A caller holding the unit's [`ObjectStore`] uses [`Self::resolve_unit`]** — the guid
    /// alone cannot name a companion pet or a guardian ([`NameKey::of`]).
    pub(crate) fn resolve(&self, guid_val: u64, commands: &NetCommands) -> Option<&str> {
        self.resolve_unit(guid_val, None, commands)
    }

    /// The name for a unit, keyed off its descriptor — the reference's `GetUnitName`
    /// (`0x609210`, see [`NameKey`]). `unit: None` degrades to [`Self::resolve`]'s guid-only key.
    /// Same ask-once discipline on every branch.
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
            // A pet is a `TYPEID_UNIT` like any creature, but one with a pet number is named
            // through its own query. Asking the creature query for the number is what left every
            // summoned pet nameless; asking the pet query for a number the server never filed is
            // what left every companion pet nameless.
            NameKey::Pet(pet_number) => self.resolve_pet(pet_number, guid_val, commands),
            NameKey::Creature(entry) => self.resolve_creature(entry, guid_val, commands),
        }
    }

    /// The name for a live pet by its `pet_number` — the pet twin of [`Self::resolve_creature`],
    /// same ask-once discipline. Unlike the creature/player queries this one has **no negative
    /// answer**: vmangos returns early without a packet when the guid is not a live pet bearing that
    /// number (`PetHandler.cpp:190-192`), so an unanswered ask simply stays unresolved rather than
    /// caching a "server doesn't know" — and is re-asked after a reconnect like the others.
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

    /// The name for a creature template `entry`, if known — the entry-keyed twin of
    /// [`Self::resolve`]'s creature branch (same cache, same ask-once discipline), for a caller that
    /// has no live spawn guid to decode an entry from: a quest objective names its kill target only
    /// by the template's raw `creature_or_go` entry (`crate::ui_quest_log`). `guid` rides along for
    /// the query body when a real spawn is known, `0` otherwise — the server answers by entry
    /// regardless of which spawn asked (the same template-only convention as
    /// [`crate::items::Items::template`]'s `guid: 0`).
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

    /// The cached name for `guid`, read-only — no query on a miss (the trace/diagnostic twin of
    /// [`Self::resolve`], for callers that must not mutate the ask-once state). Guid-only, with
    /// [`Self::resolve`]'s limits; [`Self::peek_unit`] is the descriptor-keyed twin.
    pub(crate) fn peek(&self, guid_val: u64) -> Option<&str> {
        self.peek_unit(guid_val, None)
    }

    /// The cached name for a unit keyed off its descriptor, read-only — [`Self::resolve_unit`]'s
    /// no-query twin.
    pub(crate) fn peek_unit(&self, guid_val: u64, unit: Option<&ObjectStore>) -> Option<&str> {
        match NameKey::of(guid_val, unit)? {
            NameKey::Player(guid_val) => self.players.get(guid_val).map(String::as_str),
            NameKey::Pet(pet_number) => self.pets.get(pet_number).map(String::as_str),
            NameKey::Creature(entry) => self.creatures.get(entry).map(|r| r.name.as_str()),
        }
    }

    /// Record a player-name answer. An empty wire name means the server doesn't know the guid —
    /// cached as a negative answer.
    ///
    /// `traits` is the `(race, class, gender)` triple that rides `SMSG_NAME_QUERY_RESPONSE`, kept
    /// for [`Self::player_traits`]. It is `None` for the seams that learn a name **without** that
    /// packet — the login-time seed of our own name, and the capture fixtures — rather than having
    /// them invent a zero triple that would read as a real (and wrong) race.
    ///
    /// **A `None` therefore *clears* any traits already filed under this guid**, rather than
    /// leaving them: the triple describes the character the name belongs to, and this call is
    /// replacing that name. Keeping the old one would leave a record whose name says one character
    /// and whose race says another — a contradiction manufactured by the cache itself.
    pub(crate) fn insert_player(&mut self, guid: u64, name: String, traits: Option<(u8, u8, u8)>) {
        match traits.filter(|_| !name.is_empty()) {
            Some(t) => self.player_traits.insert(guid, t),
            None => self.player_traits.remove(&guid),
        };
        self.players
            .insert(guid, (!name.is_empty()).then_some(name));
        self.generation = self.generation.wrapping_add(1);
    }

    /// **Drop everything that names one character or one spawn — the reference's world-session
    /// wipe** (`0x555740`, the `0x5557ad call 0x560400` arm for the player cache).
    ///
    /// The reference walks all thirteen `DBCache<T>` singletons at world-session start and clears
    /// each one **whose persistence is disabled** — the byte at `[cache+0x39]`, tested inverted:
    /// non-zero skips, zero clears. Four stores pass `0` to the ctor and are therefore both
    /// never-written-to-disk and wiped here: `'WNAM'` (player names, `0x554cd0` — three `push 0x0`
    /// against `'WNPC'`/`'WIDB'`'s `push 0x1; push 0x1`), `'WGLD'`, `'WPNM'` and `'WPTN'`. A real
    /// 1.12 install's `WDB/` holds exactly the enabled ones and none of those four.
    ///
    /// The rule behind the list is the key's meaning. A creature template **entry** names the same
    /// creature on every realm, forever, so its record survives both the wipe and the process. A
    /// player **guid** names one character on one realm, and a **pet number** names one live spawn
    /// — and nothing on the 1.12 wire ever says either has been handed to somebody else. So the
    /// reference does not keep them, and benilla keeping them is what let a server's brand-new
    /// character wear a deleted one's name (B386, decision 2223).
    ///
    /// Called at world entry, **before** the login seeds our own name — the seed is about the
    /// session we are starting, not the one we just left.
    pub(crate) fn clear_world_session(&mut self) {
        self.players.clear();
        self.player_traits.clear();
        self.pets.clear();
        self.generation = self.generation.wrapping_add(1);
    }

    /// **Install a realm cache read off disk — its CREATURE templates, and nothing else**
    /// (decision 2260).
    ///
    /// The persisted file has carried creature templates alone since 2223, so this is the whole of
    /// what a load can legitimately say. It used to be spelled `*names = loaded` in
    /// [`crate::name_persist::load_name_cache`], which is the same thing only when the loader is
    /// the first writer of the session — and it is not. The loader fires the first frame the realm
    /// identity is known, which is the frame the *pick* is in flight; the login's
    /// `SMSG_..._VERIFY_WORLD` seed of our OWN name ([`Self::insert_player`], from
    /// `net::session::connected`) lands in the same neighbourhood, and whichever arrives
    /// second wins. When the load won, it discarded the player's own name — so `UnitName("player")`
    /// answered nil for one wire round-trip while the feed re-asked for a guid it had just been
    /// told about, and every addon reading it in that window saw nil (the KLHThreatMeter
    /// `table index is nil` report; reproduced live 3 runs in 4).
    ///
    /// Once per process, because `NameCacheFile::realm` only goes `None` → `Some` once — which is
    /// exactly the "first login of a fresh game start, and never again" shape the report named.
    ///
    /// So the rule is not an ordering to get right, it is a *scope*: this touches what the file
    /// carries. Dropping the guid-keyed stores across a realm change is
    /// [`Self::clear_world_session`]'s, and it runs at every world entry regardless.
    ///
    /// Replace rather than merge — the realm-change case, 2223's own reasoning: the previous
    /// realm's templates are not this realm's. The generation ticks because a landed record is
    /// exactly what the gated feeds watch for (1439).
    pub(crate) fn install_persisted(&mut self, loaded: NameCache) {
        self.creatures = loaded.creatures;
        self.generation = self.generation.wrapping_add(1);
    }

    /// The landed-answer counter — see the [`Self::generation`] field.
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// `(race, class, gender)` for a player guid the name query has answered for — the macro
    /// expander's fallback when the unit isn't streamed. See [`NameCache::player_traits`].
    pub(crate) fn player_traits(&self, guid: u64) -> Option<(u8, u8, u8)> {
        self.player_traits.get(&guid).copied()
    }

    /// Record a pet-name answer (`SMSG_PET_NAME_QUERY_RESPONSE`), keyed by pet number.
    pub(crate) fn insert_pet(&mut self, pet_number: u32, name: String) {
        self.pets.insert(pet_number, Some(name));
        self.generation = self.generation.wrapping_add(1);
    }

    /// Forget a pet's cached name so the next [`Self::resolve`] asks for it again (decision 1066).
    ///
    /// **The one hole in the ask-once discipline, and the one place it has to have one.** A player's
    /// or a creature's name cannot change under a cache entry; a pet's can — its owner renames
    /// it — and nothing on the wire pushes the new one, because a pet's name does not ride its
    /// descriptor. What *does* arrive is a bumped `UNIT_FIELD_PET_NAME_TIMESTAMP`, and observing
    /// that is what calls this (`crate::ui_pet::unit`). Clearing the pending marker matters too: a
    /// rename racing an in-flight query would otherwise have its re-ask deduped away and leave the
    /// stale name in place forever.
    pub(crate) fn forget_pet(&mut self, pet_number: u32) {
        self.pets.evict(pet_number);
        self.generation = self.generation.wrapping_add(1);
    }

    /// Record a creature-name answer (`SMSG_CREATURE_QUERY_RESPONSE`); `None` = unknown entry.
    pub(crate) fn insert_creature(&mut self, entry: u32, record: Option<CreatureRecord>) {
        self.creatures.insert(entry, record);
        self.generation = self.generation.wrapping_add(1);
    }

    /// The cached subname (the overhead/tooltip title line) for a creature entry — read-only: the
    /// nameplate asks only after [`Self::resolve`] already returned the name (same answer packet).
    pub(crate) fn creature_subname(&self, entry: u32) -> Option<&str> {
        self.creatures.get(entry)?.subname.as_deref()
    }

    /// The whole cached record for a creature entry — the unit tooltip's read (subtitle, type,
    /// rank word, civilian — decision 0276's level-line law), and the one route to a live unit's
    /// [`type_flags`]. Read-only, the subname's ask-once discipline.
    ///
    /// **Key it off the unit's descriptor `OBJECT_FIELD_ENTRY`, never the entry in its guid**
    /// (decision 2068): the reference registers this record under `[[unit+8]+0xc]` (`0x60b160`),
    /// there is exactly one record per unit, and only the descriptor carries a template entry for
    /// a `HIGHGUID_PET` guid — whose entry slot holds a pet number instead. A guid-keyed twin of
    /// this read used to exist and is gone: two keys onto one field is how the readers drift.
    ///
    /// `None` = no template answer yet, and **each consumer decides what that means, because the
    /// reference's per-bit getters do**: the parked-event gate (`MORE_AUDIBLE`, decision 1482)
    /// fails CLOSED like `0x623b70`'s null leg, the wound gate (`DO_NOT_PLAY_WOUND_ANIM`, 2068)
    /// fails OPEN like `0x6125f0`'s.
    pub(crate) fn creature_record(&self, entry: u32) -> Option<&CreatureRecord> {
        self.creatures.get(entry)
    }

    /// The cached `CreatureType.dbc` id for a creature entry — read-only, same ask-once discipline
    /// as the subname (the TAB-target scan reads it; an unresolved entry is `None`, which the scan
    /// treats as targetable — the client's own out-of-range skip).
    pub(crate) fn creature_type(&self, entry: u32) -> Option<u32> {
        Some(self.creatures.get(entry)?.creature_type)
    }

    /// Forget the in-flight asks (a disconnect may have dropped them on the writer floor). Resolved
    /// player/creature names stay — those are identities, stable across sessions. Resolved **pet**
    /// names do not: a pet number names one live spawn rather than a template, and nothing that
    /// answered before the disconnect is still on the map after it.
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
// Persistence — the `.wdb` law, in benilla's own folder (decision 1689)
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// The file's first line. Every field is compared by **equality** on load and any mismatch
/// discards the whole file — the reference's own rule for its `.wdb` header, whose 20 bytes are
/// `[FourCC | build 0x16f3 | locale | recordSize | version 1]` and carry **no checksum, no
/// timestamp and no TTL**. Reproducing the *absence* matters as much as the presence: a cache
/// that expired entries on a clock would re-ask for names the server has no reason to have
/// changed, and one that trusted a checksum over a build would deserialize last patch's record
/// layout into this one's struct.
const CACHE_MAGIC: &str = "benilla-namecache";
/// Our own record-layout version — the analogue of the header's `recordSize`+`version` pair. Bump
/// it whenever a column below is added, removed or reordered; the old file is then discarded
/// rather than misparsed.
///
/// **2** — the player (`P`/`p`) and pet (`E`) records are gone (decision 2223). They were never the
/// reference's to write: `'WNAM'` and `'WPNM'` are constructed with persistence disabled and are
/// wiped at world-session start instead ([`NameCache::clear_world_session`]). A version-1 file
/// still holding them is discarded whole, which is the header's job and exactly the right thing —
/// its player lines are the stale identities B386 was about.
const CACHE_FORMAT: u32 = 2;
/// The client build the names were answered by — the reference's `0x16f3`. A different build is a
/// different server protocol, so its answers are not ours.
const CACHE_BUILD: u32 = 5875;
/// The locale the names are in. benilla is enUS-only today (every DBC read takes locale slot 0),
/// so this is a constant — but it is *in* the header because that is the reference's law, and the
/// day benilla speaks a second locale a stale enUS cache must be discarded rather than shown.
const CACHE_LOCALE: &str = "enUS";

impl NameCache {
    /// Serialize the cache as the TSV this module persists — a header line, then one line per
    /// record. Tab-separated because a name may contain spaces but never a tab or a newline, and
    /// because a player who opens `benilla-config/` should be able to read what we kept about them.
    ///
    /// **`pending_*` is deliberately not written.** An in-flight ask is a fact about this
    /// connection, not about the names; persisting it would make a cache reloaded mid-query
    /// believe an answer was coming that never will.
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

    /// Rebuild a cache from [`Self::to_tsv`]'s output, or `None` if the header does not match this
    /// build/locale/format **exactly**. A malformed or truncated line is skipped rather than
    /// failing the load: a half-written cache should cost a re-query, never a broken session.
    pub(crate) fn from_tsv(text: &str, realm: &str) -> Option<Self> {
        let mut lines = text.lines();
        let header: Vec<&str> = lines.next()?.split('\t').collect();
        // The equality gate. Five fields, all of them, no tolerance — see `CACHE_MAGIC`.
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
                            // An empty subname is NO subname, the same mapping the wire decode
                            // makes — `Some("")` would paint an empty tooltip line.
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

    /// Drop a player's cached name (`SMSG_INVALIDATE_PLAYER`) so the next resolve re-asks — the
    /// reference's remove-by-key (`0x556ff0`; eviction is **explicit only**, there is no TTL).
    ///
    /// This is the safety valve persistence needs. In memory a stale name lasts a session; on disk
    /// it lasts forever, so the one packet that says "forget this guid" has to be honoured. Note
    /// that vmangos never sends it — so against our own server this is the mechanism being present
    /// and correct rather than a path we exercise.
    pub(crate) fn invalidate_player(&mut self, guid: u64) {
        self.players.evict(guid);
        self.player_traits.remove(&guid);
        self.generation = self.generation.wrapping_add(1);
    }

    /// How many records the cache holds — the persistence layer's "is this worth writing" read.
    pub(crate) fn len(&self) -> usize {
        self.players.len() + self.creatures.len() + self.pets.len()
    }
}

/// The name-query answers (in the net handler table since 2325, moved out of the drain's names
/// arm file) — the three `*_QUERY_RESPONSE` packets that fill the ask-once cache
/// (`NameCache::resolve` is what asked for them), and the invalidation that unsticks a player's
/// name. Registered from [`crate::net::NetPlugin`], which initialises the cache.
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

    /// `SMSG_NAME_QUERY_RESPONSE` — a player's name, plus the race/gender/class that ride the same
    /// answer. Those three now have a consumer: the `$`-macro expander's subject fallback for a player
    /// who isn't streamed ([`crate::npc_text::subject_for_guid`]).
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

    /// `SMSG_PET_NAME_QUERY_RESPONSE` — keyed by pet *number*, not by template entry.
    fn pet_name(pet_number: u32, name: String, names: &mut NameCache) {
        names.insert_pet(pet_number, name);
    }

    /// `SMSG_INVALIDATE_PLAYER` — drop this guid so the next resolve re-asks (decision 1689).
    ///
    /// The name cache has **no TTL** (eviction is explicit only, `0x555600`), so this is the
    /// one thing that unsticks a player's name. It matters more to benilla than it did before the
    /// cache persisted: in memory a stale name lasted a session, on disk it lasts until something
    /// evicts it.
    fn invalidate_player(guid: u64, names: &mut NameCache) {
        debug!("net: invalidating cached name for player {guid:#x}");
        names.invalidate_player(guid);
    }

    /// `SMSG_CREATURE_QUERY_RESPONSE` — the template's name plus the hover line's fields. A `None` name
    /// is the server's "no such entry", cached as such so the ask never repeats; the remaining fields
    /// then carry their miss defaults.
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

    /// The gated feeds' counter (1439): a landed answer (and the rename eviction) moves it; an
    /// ask-once miss — the read gated feeds run every frame — never does, or the outstanding
    /// query would hold every name-watching gate open until it answered.
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

    /// A summoned pet resolves through `CMSG_PET_NAME_QUERY`, keyed by the pet number in its guid —
    /// NOT through the creature query, which would ask for a template entry that does not exist and
    /// leave the pet nameless (the reported bug: an NPC-summoned voidwalker with no name).
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
        // Ask-once, like every other name.
        assert_eq!(cache.resolve(voidwalker, &cmds), None);
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));

        cache.insert_pet(137, "Voidwalker".into());
        assert_eq!(cache.resolve(voidwalker, &cmds), Some("Voidwalker"));
        assert_eq!(cache.peek(voidwalker), Some("Voidwalker"));
        // Pet names are per-spawn, so a disconnect drops them.
        cache.clear_pending();
        assert_eq!(cache.peek(voidwalker), None);
    }

    /// **A stable list warms the pet-name cache, so an unstabled pet is named on arrival**
    /// (decision 1688). The list carries the same `(pet_number, name)` pair the pet-name query
    /// would answer with, and the guid slot benilla reads that number out of is the very one
    /// vmangos packs it into (`Object::_Create(guidlow, petNumber, HIGHGUID_PET)`).
    ///
    /// The window this closes is a Lua error the director hit: `PetStable_Update` hands a bare
    /// `UnitName("pet")` to `GameTooltip:SetText`, whose byte-pinned signature REQUIRES a string
    /// and raises on nil. The reference's own line is unguarded too — it survives because
    /// `petnamecache.wdb` persists across sessions and benilla has no such file.
    #[test]
    fn a_stable_list_names_the_pet_before_it_is_ever_summoned() {
        let (cmds, rx) = commands();
        let cache = NameCache::default();
        // The guid the pet will have once summoned: its pet number rides the ENTRY slot.
        let rex = compose(guid::HIGH_PET, 7, 42);

        // Cold: nil, and an ask goes out — the state that raised the error.
        assert_eq!(cache.resolve(rex, &cmds), None);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::PetNameQuery { pet_number: 7, .. })
        ));

        // The list lands and seeds the name. Now the pet resolves with NO further query, which is
        // the whole point: it is named the frame it is summoned, not a round trip later.
        let mut warm = NameCache::default();
        warm.insert_pet(7, "Rex".into());
        assert_eq!(warm.resolve(rex, &cmds), Some("Rex"));
        assert!(
            matches!(rx.try_recv(), Err(TryRecvError::Empty)),
            "a seeded pet must not re-ask"
        );
    }

    /// **The round trip keeps every record kind and every shape of answer** (decision 1689) —
    /// including the "the server said no" negative, which is the one a naive serializer drops:
    /// cached as absent, it is what stops us re-asking an unknown entry on every login, forever.
    ///
    /// The *player* negative used to be here too and is gone with the player records themselves
    /// (2223): a guid the server did not know this week is not a guid it will not know next week,
    /// because the guid may by then belong to somebody it does.
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

        // A cached negative must survive as a negative — present-and-None, not absent.
        assert!(back.creatures.answered(1234));
        let rec = back.creature_record(69).expect("creature survived");
        assert_eq!(rec.name, "Stable Master Kitrik");
        assert_eq!(rec.subname.as_deref(), Some("Stable Master"));
        assert_eq!((rec.creature_type, rec.rank, rec.type_flags), (7, 1, 0x10));
        assert_eq!((rec.civilian, rec.racial_leader), (true, false));
        assert_eq!(rec.display_id, 533, "the model a stabled pet is drawn from");
        assert!(back.creature_record(1234).is_none());
        assert_eq!(back.len(), 2);

        // An EMPTY subname must come back as no subname, the same mapping the wire decode makes —
        // `Some("")` paints an empty tooltip line whose zero-extent slot spills the ones below it.
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

    /// **B386 — the file carries creature templates and NOTHING that names one character.**
    ///
    /// The reference constructs `'WNAM'` (player names) and `'WPNM'` (pet names) with persistence
    /// **disabled** — `0x554cd0`'s three `push 0x0` against `'WNPC'`/`'WIDB'`'s `push 0x1; push
    /// 0x1` — and a real 1.12 install's `WDB/` holds `creaturecache.wdb` and no `namecache.wdb`.
    /// A creature template entry means the same creature forever; a player guid means one
    /// character on one realm, and a wiped server hands it to the next one it creates.
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

    /// **World entry drops every guid- and spawn-keyed name, and keeps the templates** — the
    /// reference's `0x555740`, which clears exactly the stores whose `[cache+0x39]` enable byte is
    /// zero (tested inverted: `0x55579f`–`0x5557ad` for the player cache at `0xc0e228`).
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
        // And the ask-once markers went with them, so the next sighting asks rather than sitting
        // behind a pending flag from a session that is over.
        assert_eq!(cache.resolve(0x11, &cmds), None);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::NameQuery { guid: 0x11 })
        ));
    }

    /// **Replacing a name drops the traits that belonged to the old one.** They describe the
    /// character the name names; leaving them behind manufactures exactly the record the
    /// descriptor check exists to catch — one whose name says one character and whose race says
    /// another.
    #[test]
    fn a_name_replaced_without_traits_does_not_keep_the_old_ones() {
        let mut cache = NameCache::default();
        cache.insert_player(0x7, "Moamtester".into(), Some((1, 1, 0)));
        cache.insert_player(0x7, "Corances".into(), None);
        assert_eq!(cache.player_traits(0x7), None);
    }

    /// **The header is an equality gate with no tolerance** — the reference's own `.wdb` rule, and
    /// the reason its cache carries no checksum and no clock. Each field is rejected on its own:
    /// a different build speaks a different protocol, a different locale means different strings,
    /// a different format version means these columns are not those columns, and a different realm
    /// means every key in the file addresses somebody else.
    #[test]
    fn a_header_that_differs_in_any_field_discards_the_whole_file() {
        let mut cache = NameCache::default();
        cache.insert_creature(1234, None);
        let good = cache.to_tsv("Hydraxian Waterlords");
        assert!(NameCache::from_tsv(&good, "Hydraxian Waterlords").is_some());

        // The realm — the field the reference does NOT have, and the one that would silently
        // serve another realm's names.
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

        // Garbage and truncation cost a re-query, never a broken session.
        assert!(NameCache::from_tsv("", "R").is_none());
        assert!(NameCache::from_tsv("nonsense", "R").is_none());
        // A good header with a mangled record line keeps the header's other records.
        let text = format!("{}\nC\tnotanumber\t1\t2\t0\tX\nc\t1234\n", head);
        let back = NameCache::from_tsv(&text, "Hydraxian Waterlords").expect("header still good");
        assert_eq!(back.len(), 1, "the bad line is skipped, the good one kept");

        // **A version-1 file is discarded whole, and its player lines with it** (2223). The format
        // field does that on its own — but say it here, because those lines are exactly the stale
        // identities B386 was about and "the old file is simply dropped" is the property that
        // matters, not that the `P` arm no longer parses.
        let v1 = format!(
            "{CACHE_MAGIC}\t1\t{CACHE_BUILD}\t{CACHE_LOCALE}\tHydraxian Waterlords\n\
             P\t7\t1\t1\t0\tMoamtester\n"
        );
        assert!(NameCache::from_tsv(&v1, "Hydraxian Waterlords").is_none());
    }

    /// `SMSG_INVALIDATE_PLAYER`'s eviction — the safety valve persistence needs. The cache ages
    /// nothing out, so without this a name written to disk would outlive any change to it.
    #[test]
    fn invalidating_a_player_makes_the_next_resolve_ask_again() {
        let (cmds, rx) = commands();
        let mut cache = NameCache::default();
        cache.insert_player(0x11, "Sam".into(), Some((1, 2, 0)));
        assert_eq!(cache.resolve(0x11, &cmds), Some("Sam"));
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

        // The next resolve is a genuine miss again — it asks.
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
        // …and no re-ask.
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn clear_pending_allows_a_reconnect_reask() {
        let (cmds, rx) = commands();
        let mut cache = NameCache::default();
        let g = compose(guid::HIGH_UNIT, 100, 1);

        assert_eq!(cache.resolve(g, &cmds), None);
        let _ = rx.try_recv();
        // The answer never lands (disconnect); pending cleared → the next resolve re-asks.
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

    /// **A companion pet is named by its template, not by the pet number in its guid** —
    /// `0x609210`'s `UNIT_FIELD_PETNUMBER == 0` leg. vmangos builds a mini-pet as a
    /// `HIGHGUID_PET` guid around a fresh pet number but never files that number in its charm
    /// info, so a `CMSG_PET_NAME_QUERY` for it is never answered (`PetHandler.cpp:190-192`) — the
    /// guid-keyed resolve left every Mechanical Squirrel "Unknown" forever. Its descriptor says
    /// PETNUMBER 0, and the reference reads the creature record under `OBJECT_FIELD_ENTRY`.
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

    /// The cold companion asks the CREATURE query for its descriptor entry — once.
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

    /// A hunter/warlock pet carries its pet number in the descriptor (`SetPetNumber(n, true)`),
    /// and that is the key the pet-name query goes out under — not the creature template.
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

    /// A mind-controlled creature (`HIGHGUID_UNIT` guid) carries a pet number too
    /// (`SpellAuras.cpp:3248`, `SetPetNumber(GeneratePetNumber(), true)`), and the reference names
    /// it through the pet-name cache under that number for as long as the charm holds.
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
