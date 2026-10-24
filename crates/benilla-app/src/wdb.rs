//! The template caches kept between sessions, and the caches kept for one world session.
//!
//! The reference holds thirteen `DBCache` stores of one template, each built with a persist byte
//! (`[cache+0x39]`, stored at `0x55e770`), its constructor's last argument (`0x554b40` and its
//! kin). Its load (`0x5572b0` and kin, all run once at start-up by `0x554f50`) and its save (the
//! store's `_atexit` writer, `0x554b60` → `0x5566d0`) return at once when the byte is 0
//! (`0x55fa74`, `0x55ec7d`). A record lives until the server refuses its key, which frees the
//! node (a top-bit key, `0x556e67` → `0x556f35`; a quest's empty title, `0x563150`), or until the
//! file's header stops matching; nothing evicts by age or size, and a hit returns the record and
//! sends nothing (`0x5586ca` → `0x55876c`). The world entry (`CGGameUI::InitializeGame`, `0x48f586`
//! → `0x555740`) clears every store whose byte is 0.
//!
//! | store | reference file | here |
//! |---|---|---|
//! | creatures `0xc0e354` | `creaturecache.wdb` | saved, [`NameCache`] |
//! | game objects `0xc0e318` | `gameobjectcache.wdb` | saved, [`GameObjectTemplates`] |
//! | item names `0xc0e2dc` | `itemnamecache.wdb` | saved, [`ItemNames`] |
//! | items `0xc0e2a0` | `itemcache.wdb` | saved, [`Items`] |
//! | NPC text `0xc0e264` | `npccache.wdb` | saved, [`GossipState`] |
//! | quests `0xc0e1b0` | `questcache.wdb` | saved, [`QuestLog`] |
//! | page text `0xc0e174` | `pagetextcache.wdb` | saved, [`PageTexts`] |
//! | item text `0xc0e0c0` | `itemtextcache.wdb` | saved, [`MailOpen`] |
//! | `0xc0e084` | `wowcache.wdb` | none: it has no query (opcode 0) |
//! | player names `0xc0e228` | `namecache.wdb`, unused | world session, [`NameCache`] |
//! | guilds `0xc0e1ec` | `guildcache.wdb`, unused | world session, [`GuildState`] |
//! | pet names `0xc0e138` | `petnamecache.wdb`, unused | world session, [`NameCache`] |
//! | petitions `0xc0e0fc` | `petitioncache.wdb`, unused | world session, [`PetitionRecords`] |
//!
//! A file is the reference's layout (`0x5566d0`): a header, then `{key, size, record}` entries
//! ending in two zero dwords. The header is the store's tag, the build `0x16f3`, the locale tag
//! (`[0xc2a2a4]`, `0x5567d9`), our format and the realm, and a load that finds any of them
//! different keeps nothing, as the reference's does (`0x557390`-`0x55742e`). A record is the
//! server's answer as it came off the wire ([`SessionEvent::SavedAnswer`]), the key and then the
//! fields in the order the store's reader takes them (`0x7c8120` and its kin), and a load decodes
//! it with the live answer's decoder, so one mechanism serves every store and a decoder fix
//! reaches the records already on disk. A short read keeps nothing (`0x557665`).
//!
//! Deviation: the files are `benilla-config/cache/<realm>/<store>.bin` through
//! [`crate::local_state`], not `WDB/` in the install, which is read-only; they are per realm,
//! because another server may hold another template under the same key; and a store is written
//! ten seconds after an answer lands as well as at exit, so a crash costs only re-asking.

use std::collections::HashMap;
use std::marker::PhantomData;
use std::path::PathBuf;

use benilla_protocol::messages::opcode;
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use crate::glue_session::roster::Roster;
use crate::go_templates::GameObjectTemplates;
use crate::item_names::ItemNames;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::NetHandlerApp;
use crate::query_cache::QueryCache;
use crate::ui_gossip::GossipState;
use crate::ui_guild::GuildState;
use crate::ui_item_text::PageTexts;
use crate::ui_mail::MailOpen;
use crate::ui_petition::PetitionRecords;
use crate::ui_quest_log::QuestLog;

/// Seconds between writes: answers arrive in bursts on zone-in, and a crash costs only re-asking.
const SAVE_DEBOUNCE: f32 = 10.0;

/// The client build, the header's second dword (`0x5567cc`).
const BUILD: u32 = 0x16f3;

/// Our container's version, the header's fourth dword: bump it when the layout changes. The
/// records need none, being the wire's.
const FORMAT: u32 = 1;

/// A store the reference saves: the answer whose body is its record, and how a record read off
/// disk is filed.
pub(crate) trait SavedCache: Resource + Default {
    /// The reference's file name, without `.wdb`.
    const FILE: &'static str;
    /// The reference's tag, the header's first dword (the constructor's first argument, `BOMW` for
    /// creatures at `0x554b4d`).
    const TAG: u32;
    /// The answer whose body is the record.
    const ANSWER: u16;
    type Record;
    fn store(&self) -> &QueryCache<u32, Self::Record>;
    fn store_mut(&mut self) -> &mut QueryCache<u32, Self::Record>;
    /// Files one decoded answer read off disk the way its live handler files the record, with no
    /// lookup waiting on it.
    fn install(&mut self, answer: SessionEvent);
    /// Drops every record, for another realm's file.
    fn forget(&mut self) {
        self.store_mut().clear();
    }
}

impl SavedCache for NameCache {
    const FILE: &'static str = "creaturecache";
    const TAG: u32 = 0x574d_4f42;
    const ANSWER: u16 = opcode::SMSG_CREATURE_QUERY_RESPONSE;
    type Record = crate::names::CreatureRecord;
    fn store(&self) -> &QueryCache<u32, Self::Record> {
        self.creature_store()
    }
    fn store_mut(&mut self) -> &mut QueryCache<u32, Self::Record> {
        self.creature_store_mut()
    }
    fn install(&mut self, answer: SessionEvent) {
        crate::names::net::file_creature(answer, self);
    }
}

impl SavedCache for GameObjectTemplates {
    const FILE: &'static str = "gameobjectcache";
    const TAG: u32 = 0x5747_4f42;
    const ANSWER: u16 = opcode::SMSG_GAMEOBJECT_QUERY_RESPONSE;
    type Record = crate::go_templates::GoTemplate;
    fn store(&self) -> &QueryCache<u32, Self::Record> {
        self.template_store()
    }
    fn store_mut(&mut self) -> &mut QueryCache<u32, Self::Record> {
        self.template_store_mut()
    }
    fn install(&mut self, answer: SessionEvent) {
        if let SessionEvent::GameObjectInfo {
            entry,
            type_id,
            name,
            data,
            ..
        } = answer
        {
            self.insert(entry, type_id, name, &data);
        }
    }
}

impl SavedCache for ItemNames {
    const FILE: &'static str = "itemnamecache";
    const TAG: u32 = 0x574e_4442;
    const ANSWER: u16 = opcode::SMSG_ITEM_NAME_QUERY_RESPONSE;
    type Record = String;
    fn store(&self) -> &QueryCache<u32, Self::Record> {
        ItemNames::store(self)
    }
    fn store_mut(&mut self) -> &mut QueryCache<u32, Self::Record> {
        ItemNames::store_mut(self)
    }
    fn install(&mut self, answer: SessionEvent) {
        if let SessionEvent::ItemName {
            entry,
            name: Some(name),
        } = answer
        {
            self.file(entry, Some(name));
        }
    }
}

impl SavedCache for Items {
    const FILE: &'static str = "itemcache";
    const TAG: u32 = 0x5749_4442;
    const ANSWER: u16 = opcode::SMSG_ITEM_QUERY_SINGLE_RESPONSE;
    type Record = benilla_protocol::ItemInfo;
    fn store(&self) -> &QueryCache<u32, Self::Record> {
        self.template_store()
    }
    fn store_mut(&mut self) -> &mut QueryCache<u32, Self::Record> {
        self.template_store_mut()
    }
    fn install(&mut self, answer: SessionEvent) {
        if let SessionEvent::ItemTemplate {
            entry,
            info: Some(info),
        } = answer
        {
            self.insert_template(entry, Some(*info));
        }
    }
    /// The interface's copy drops them too.
    fn forget(&mut self) {
        self.forget_templates();
    }
}

impl SavedCache for GossipState {
    const FILE: &'static str = "npccache";
    const TAG: u32 = 0x574e_5043;
    const ANSWER: u16 = opcode::SMSG_NPC_TEXT_UPDATE;
    type Record = Vec<benilla_protocol::messages::NpcTextBlock>;
    fn store(&self) -> &QueryCache<u32, Self::Record> {
        self.record_store()
    }
    fn store_mut(&mut self) -> &mut QueryCache<u32, Self::Record> {
        self.record_store_mut()
    }
    fn install(&mut self, answer: SessionEvent) {
        if let SessionEvent::NpcGreeting { text_id, blocks } = answer {
            self.remember_record(text_id, blocks);
        }
    }
}

impl SavedCache for QuestLog {
    const FILE: &'static str = "questcache";
    const TAG: u32 = 0x5751_5354;
    const ANSWER: u16 = opcode::SMSG_QUEST_QUERY_RESPONSE;
    type Record = benilla_protocol::messages::QuestTemplate;
    fn store(&self) -> &QueryCache<u32, Self::Record> {
        self.template_store()
    }
    fn store_mut(&mut self) -> &mut QueryCache<u32, Self::Record> {
        self.template_store_mut()
    }
    fn install(&mut self, answer: SessionEvent) {
        if let SessionEvent::QuestTemplate(template) = answer {
            self.insert_template(*template);
        }
    }
}

impl SavedCache for PageTexts {
    const FILE: &'static str = "pagetextcache";
    const TAG: u32 = 0x5750_5458;
    const ANSWER: u16 = opcode::SMSG_PAGE_TEXT_QUERY_RESPONSE;
    type Record = crate::ui_item_text::PageText;
    fn store(&self) -> &QueryCache<u32, Self::Record> {
        self.page_store()
    }
    fn store_mut(&mut self) -> &mut QueryCache<u32, Self::Record> {
        self.page_store_mut()
    }
    fn install(&mut self, answer: SessionEvent) {
        if let SessionEvent::PageText {
            page_id,
            text,
            next_page_id,
        } = answer
        {
            self.insert(page_id, text, next_page_id);
        }
    }
}

impl SavedCache for MailOpen {
    const FILE: &'static str = "itemtextcache";
    const TAG: u32 = 0x5749_5458;
    const ANSWER: u16 = opcode::SMSG_ITEM_TEXT_QUERY_RESPONSE;
    type Record = String;
    fn store(&self) -> &QueryCache<u32, Self::Record> {
        &self.bodies
    }
    fn store_mut(&mut self) -> &mut QueryCache<u32, Self::Record> {
        &mut self.bodies
    }
    fn install(&mut self, answer: SessionEvent) {
        if let SessionEvent::MailItemText { text_id, text } = answer {
            self.bodies.insert(text_id, text);
        }
    }
}

/// The answers whose bodies the IO thread hands on ([`SessionEvent::SavedAnswer`]).
const SAVED_ANSWERS: [u16; 8] = [
    NameCache::ANSWER,
    GameObjectTemplates::ANSWER,
    ItemNames::ANSWER,
    Items::ANSWER,
    GossipState::ANSWER,
    QuestLog::ANSWER,
    PageTexts::ANSWER,
    MailOpen::ANSWER,
];

/// Whether `opcode` answers a saved store's query.
pub(crate) fn is_saved_answer(opcode: u16) -> bool {
    SAVED_ANSWERS.contains(&opcode)
}

pub(crate) struct WdbPlugin;

impl Plugin for WdbPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Roster>();
        saved::<NameCache>(app);
        saved::<GameObjectTemplates>(app);
        saved::<ItemNames>(app);
        saved::<Items>(app);
        saved::<GossipState>(app);
        saved::<QuestLog>(app);
        saved::<PageTexts>(app);
        saved::<MailOpen>(app);
        app.init_resource::<GuildState>()
            .init_resource::<PetitionRecords>()
            .net_handler(SessionEventKind::SavedAnswer, keep_answer)
            .net_handler(SessionEventKind::Connected, clear_session_stores);
    }
}

/// A saved store's load and debounced write, ahead of `Update`'s readers, and its exit write.
fn saved<T: SavedCache>(app: &mut App) {
    app.init_resource::<T>()
        .init_resource::<WdbFile<T>>()
        .add_systems(PreUpdate, (load::<T>, save::<T>).chain());
    // The exit write catches the last burst, which the debounce would not.
    crate::shutdown::on_app_exit(app, save_on_exit::<T>.into_configs());
}

/// The world entry's clear of the guild and petition stores, built with persistence off
/// (`0x554d20`, `0x554e60`; `0x5557c0`, `0x55580c`). The player and pet names clear in the world
/// entry's own handler, ahead of the seed of our own name (`crate::net`).
fn clear_session_stores(
    In(ev): In<SessionEvent>,
    mut guilds: ResMut<GuildState>,
    mut petitions: ResMut<PetitionRecords>,
) {
    if let SessionEvent::Connected { .. } = ev {
        guilds.clear_world_session();
        petitions.clear_world_session();
    }
}

/// Every saved store's file, for the answer bytes' handler.
#[derive(bevy::ecs::system::SystemParam)]
struct Files<'w> {
    creatures: ResMut<'w, WdbFile<NameCache>>,
    game_objects: ResMut<'w, WdbFile<GameObjectTemplates>>,
    item_names: ResMut<'w, WdbFile<ItemNames>>,
    items: ResMut<'w, WdbFile<Items>>,
    npc_text: ResMut<'w, WdbFile<GossipState>>,
    quests: ResMut<'w, WdbFile<QuestLog>>,
    pages: ResMut<'w, WdbFile<PageTexts>>,
    item_text: ResMut<'w, WdbFile<MailOpen>>,
}

/// An answer's bytes, to the file of the store it answers.
fn keep_answer(In(ev): In<SessionEvent>, mut files: Files) {
    let SessionEvent::SavedAnswer { opcode, body } = ev else {
        return;
    };
    match opcode {
        NameCache::ANSWER => files.creatures.keep(body),
        GameObjectTemplates::ANSWER => files.game_objects.keep(body),
        ItemNames::ANSWER => files.item_names.keep(body),
        Items::ANSWER => files.items.keep(body),
        GossipState::ANSWER => files.npc_text.keep(body),
        QuestLog::ANSWER => files.quests.keep(body),
        PageTexts::ANSWER => files.pages.keep(body),
        MailOpen::ANSWER => files.item_text.keep(body),
        _ => {}
    }
}

/// One store's file: the realm it belongs to, the answer bytes of its records, and the dirty
/// mark.
#[derive(Resource)]
pub(crate) struct WdbFile<T> {
    /// The realm the loaded file belongs to; a change saves the old one and loads the new.
    realm: Option<String>,
    path: Option<PathBuf>,
    /// Each record's answer as it came off the wire, by key. A body whose record the store no
    /// longer holds is not written.
    bodies: HashMap<u32, Vec<u8>>,
    /// The store's generation at the last write.
    saved: u64,
    /// Bytes have landed since the last write, which the generation does not count.
    dirty: bool,
    since_save: f32,
    _store: PhantomData<T>,
}

impl<T> Default for WdbFile<T> {
    fn default() -> Self {
        Self {
            realm: None,
            path: None,
            bodies: HashMap::new(),
            saved: 0,
            dirty: false,
            since_save: 0.0,
            _store: PhantomData,
        }
    }
}

/// The key a body opens with, which `Receive` reads first (`0x556e5b`).
fn body_key(body: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(body.get(..4)?.try_into().ok()?))
}

impl<T> WdbFile<T> {
    /// Keeps an answer's bytes for its record; a refusal's top-bit key (`0x556e67`) drops them.
    fn keep(&mut self, body: Vec<u8>) {
        let Some(key) = body_key(&body) else {
            return;
        };
        if key & 0x8000_0000 != 0 {
            self.bodies.remove(&(key & 0x7fff_ffff));
        } else {
            self.bodies.insert(key, body);
        }
        self.dirty = true;
    }
}

/// Loads the realm's file the first frame its identity is known, and on a realm change, after
/// saving the realm left.
fn load<T: SavedCache>(roster: Res<Roster>, mut file: ResMut<WdbFile<T>>, mut cache: ResMut<T>) {
    let Some((realm, _)) = crate::ui_macro::identity(&roster) else {
        return;
    };
    if file.realm.as_deref() == Some(realm.as_str()) {
        return;
    }
    if file.realm.is_some() {
        write_now(&mut file, &cache);
        cache.forget();
        file.bodies.clear();
        file.dirty = false;
    }
    file.realm = Some(realm.clone());
    file.path = crate::local_state::template_cache_path(&realm, T::FILE);
    // An earlier build's creature names, which the creature store's file replaces.
    if let Some(retired) = crate::local_state::retired_name_cache_path(&realm) {
        let _ = std::fs::remove_file(retired);
    }
    file.saved = cache.store().generation();
    let Some(path) = file.path.clone() else {
        return; // a hermetic capture, or no state folder: session-only
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return; // no file yet
    };
    let Some(entries) = decode(&bytes, T::TAG, &realm) else {
        // The first save overwrites it.
        debug!(
            "wdb: discarding {} — not this build, locale or realm",
            path.display()
        );
        return;
    };
    let n = entries.len();
    for (key, body) in entries {
        // An answer that landed before the load is newer than the file's.
        if cache.store().get(key).is_some() {
            continue;
        }
        if let Ok(packet) = benilla_protocol::messages::parse_server(T::ANSWER, &body) {
            for ev in benilla_protocol::decode(packet) {
                cache.install(ev);
            }
        }
        if cache.store().get(key).is_some() {
            file.bodies.insert(key, body);
        }
    }
    file.saved = cache.store().generation();
    debug!("wdb: loaded {n} {} records for {realm}", T::FILE);
}

/// Writes the store when its records have changed and the debounce has elapsed.
fn save<T: SavedCache>(time: Res<Time>, mut file: ResMut<WdbFile<T>>, cache: Res<T>) {
    file.since_save += time.delta_secs();
    if file.since_save < SAVE_DEBOUNCE {
        return;
    }
    file.since_save = 0.0;
    write_now(&mut file, &cache);
}

/// The exit write, regardless of the debounce.
fn save_on_exit<T: SavedCache>(mut file: ResMut<WdbFile<T>>, cache: Res<T>) {
    write_now(&mut file, &cache);
}

fn write_now<T: SavedCache>(file: &mut WdbFile<T>, cache: &T) {
    let generation = cache.store().generation();
    if generation == file.saved && !file.dirty {
        return;
    }
    let (Some(path), Some(realm)) = (file.path.as_ref(), file.realm.as_ref()) else {
        return;
    };
    let mut keys: Vec<u32> = cache.store().iter().map(|(&k, _)| k).collect();
    keys.sort_unstable();
    let entries = keys
        .into_iter()
        .filter_map(|k| file.bodies.get(&k).map(|b| (k, b.as_slice())));
    let bytes = encode(T::TAG, realm, entries);
    match crate::local_state::write_atomic_bytes(path, &bytes) {
        Ok(()) => {
            file.saved = generation;
            file.dirty = false;
            debug!("wdb: wrote {}", path.display());
        }
        Err(e) => warn!("wdb: cannot write {}: {e}", path.display()),
    }
}

/// The header's locale, the client's locale tag as each `.wdb` header carries it (`[0xc2a2a4]`,
/// written at `0x5567d9` and compared at `0x5573c6`): the server answers in the locale the logon
/// challenge sent, so another locale's file is discarded.
fn locale_tag() -> u32 {
    crate::ui_script::client_locale().tag()
}

/// A store's file: the header, each `{key, size, body}` entry, then two zero dwords.
fn encode<'a>(tag: u32, realm: &str, entries: impl Iterator<Item = (u32, &'a [u8])>) -> Vec<u8> {
    let mut out = Vec::new();
    for word in [tag, BUILD, locale_tag(), FORMAT, realm.len() as u32] {
        out.extend_from_slice(&word.to_le_bytes());
    }
    out.extend_from_slice(realm.as_bytes());
    for (key, body) in entries {
        out.extend_from_slice(&key.to_le_bytes());
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(body);
    }
    out.extend_from_slice(&[0; 8]);
    out
}

/// The next little-endian dword of `r`, consumed.
fn take_u32(r: &mut &[u8]) -> Option<u32> {
    let (head, rest) = r.split_first_chunk::<4>()?;
    *r = rest;
    Some(u32::from_le_bytes(*head))
}

/// [`encode`]'s entries, `None` when any header field differs or the file is cut short.
fn decode(bytes: &[u8], tag: u32, realm: &str) -> Option<Vec<(u32, Vec<u8>)>> {
    let mut r = bytes;
    let header = [
        take_u32(&mut r)?,
        take_u32(&mut r)?,
        take_u32(&mut r)?,
        take_u32(&mut r)?,
    ];
    if header != [tag, BUILD, locale_tag(), FORMAT] {
        return None;
    }
    let len = take_u32(&mut r)? as usize;
    if r.get(..len)? != realm.as_bytes() {
        return None;
    }
    r = &r[len..];
    let mut entries = Vec::new();
    loop {
        let key = take_u32(&mut r)?;
        let size = take_u32(&mut r)? as usize;
        if key == 0 {
            return Some(entries);
        }
        entries.push((key, r.get(..size)?.to_vec()));
        r = &r[size..];
    }
}

#[cfg(test)]
mod tests;
